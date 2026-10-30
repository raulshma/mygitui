//! POSIX shell quoting for remote command construction.
//!
//! russh `exec` sends a command STRING that the remote login shell parses
//! (there is no argv array on the wire). Every path, ref, or free-form value
//! interpolated into a remote command MUST pass through [`shell_quote`] —
//! a repo path like `my files; rm -rf ~` must stay one argument. Commands
//! are otherwise built from fixed literals we control.

/// Quote one argument for a POSIX shell (sh/bash/zsh): wrap in single
/// quotes, escaping embedded single quotes (`'` → `'\''`). Safe for empty
/// strings (yields `''`).
pub fn shell_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Quote and join arguments with single spaces.
pub fn shell_quote_all<I, S>(args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    args.into_iter()
        .map(|a| shell_quote(a.as_ref()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_word_is_wrapped() {
        assert_eq!(shell_quote("src/main.rs"), "'src/main.rs'");
    }

    #[test]
    fn empty_string_is_explicit_empty_arg() {
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn single_quote_is_escaped() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn metacharacters_stay_inside_quotes() {
        // No single quotes in the input → a plain single-quote wrap keeps
        // every metacharacter literal.
        assert_eq!(
            shell_quote("a;rm -rf ~ && echo $HOME `id` $(id) * | ><"),
            "'a;rm -rf ~ && echo $HOME `id` $(id) * | ><'"
        );
    }

    /// Naive POSIX unquoting (inverse of `shell_quote`), used to verify
    /// quoted arguments survive a shell round-trip. Understands exactly
    /// what `shell_quote` emits: quoted runs plus `'\''` escapes.
    fn shell_unquote(q: &str) -> String {
        let mut out = String::new();
        let mut in_quotes = false;
        let mut chars = q.chars().peekable();
        while let Some(c) = chars.next() {
            if in_quotes {
                if c == '\'' {
                    in_quotes = false;
                } else {
                    out.push(c);
                }
            } else if c == '\'' {
                in_quotes = true;
            } else if c == '\\' {
                // An escaped literal (the quote in a `'\''` sequence).
                if let Some(next) = chars.next() {
                    out.push(next);
                }
            }
        }
        out
    }

    #[test]
    fn injection_attempt_is_inert() {
        // The quoted result must survive a shell unquote round-trip exactly.
        for evil in [
            "x'; rm -rf /; echo '",
            "a;rm -rf ~ && echo $HOME `id` $(id)",
            "path with spaces/and's Quotes.txt",
            "",
            "'",
            "''",
        ] {
            assert_eq!(
                shell_unquote(&shell_quote(evil)),
                evil,
                "roundtrip {evil:?}"
            );
        }
    }

    #[test]
    fn join_produces_command_line() {
        let cmd = format!("git -C {} status --porcelain", shell_quote("/a b/c"));
        assert_eq!(cmd, "git -C '/a b/c' status --porcelain");
        assert_eq!(
            shell_quote_all(["--", "a b", "it's"]),
            "'--' 'a b' 'it'\\''s'"
        );
    }
}
