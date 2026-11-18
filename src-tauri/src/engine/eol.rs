//! eol awareness for the engine's manual workdir/index writes (phase 5
//! item 49).
//!
//! libgit2 applies the crlf filter (`crlf.c`) for us on `checkout_tree`,
//! `index.add_path` and the workdir side of diffs — but the mutation lanes
//! that write raw bytes themselves (discard restores, stash blob restores,
//! conflict-resolution writes, untracked-hunk staging) would bypass it. The
//! helpers here apply the same conversion those writes need, with pragmatic
//! parity with git's checkout (smudge) and check-in (clean) filters:
//!
//! * conversion is CONFIG-DRIVEN, never host-driven: `core.autocrlf=true`
//!   converts on every platform (git's `autocrlf=true` also smudges to CRLF
//!   on checkout everywhere; only the `text` attribute's "native eol" rule
//!   is host-dependent, and that is simplified away — see below);
//! * `core.autocrlf=input` strips CRLF on the way INTO the index only (git
//!   parity — input never adds CRLF on checkout);
//! * an unset/false `core.autocrlf` converts nothing (byte-exact writes, the
//!   historical behavior);
//! * binary content (NUL byte in the first 8000 bytes, git's
//!   `buffer_is_binary` heuristic) is never touched;
//! * workdir conversion skips content that already carries any CR (git's
//!   "safe autocrlf" rule from libgit2's `crlf_apply_to_workdir`: no bare
//!   LF→CRLF rewrite of mixed files);
//! * `.gitattributes` of the repo ROOT only (a minimal parser, the same
//!   dialect family as the FE `attributesModel.ts`): `text` / `-text` /
//!   `text=auto`, `eol=lf` / `eol=crlf`, and the `binary` macro (treated as
//!   `-text`). Nested per-directory attribute files, `info/attributes`,
//!   `include` directives, other macros and the legacy `crlf` attribute are
//!   unsupported simplifications. Plain `text` / `text=auto` without `eol`
//!   follow `core.autocrlf` instead of git's host-native eol rule.
//!
//! Scope note: tracked-file hunk staging needs no help here — libgit2's diff
//! already runs the TO_ODB filter on the workdir side, so the rendered patch
//! (and therefore the applied index blob) is LF under autocrlf; only the
//! untracked synthesis path reads RAW workdir bytes and calls
//! [`convert_to_index`]. git's index-side safety check (`has_cr_in_index`
//! only skips conversion when the stored blob already carries CR) is
//! therefore inert for this helper's one caller: an untracked path has no
//! stored blob.

use git2::Repository;

/// git's `buffer_is_binary` window.
const BINARY_SNIFF_BYTES: usize = 8000;

/// Value of the `text` attribute for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum TextAttr {
    /// No matching rule mentions `text`.
    #[default]
    Unset,
    /// `text` set (forced conversion).
    True,
    /// `-text` (never convert; also the `binary` macro).
    False,
    /// `text=auto` (convert like autocrlf, binary-safe).
    Auto,
}

/// Value of the `eol` attribute for one path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EolAttr {
    Lf,
    Crlf,
}

/// `core.autocrlf` as git's three-state setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoCrlf {
    Unset,
    True,
    Input,
    False,
}

/// The path's crlf-relevant attribute mentions.
#[derive(Debug, Default, PartialEq, Eq)]
struct EolAttrs {
    text: TextAttr,
    eol: Option<EolAttr>,
}

impl EolAttrs {
    /// True when the attributes force the never-convert rule (`-text` or
    /// the `binary` macro).
    fn is_binary_attr(&self) -> bool {
        self.text == TextAttr::False
    }
}

/// Read `core.autocrlf` from the repo config (missing/unreadable config =
/// unset, i.e. no conversion).
fn autocrlf(repo: &Repository) -> AutoCrlf {
    let Ok(config) = repo.config() else {
        return AutoCrlf::Unset;
    };
    match config.get_string("core.autocrlf") {
        Ok(value) => match value.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => AutoCrlf::True,
            "input" => AutoCrlf::Input,
            "false" | "0" | "no" | "off" => AutoCrlf::False,
            _ => AutoCrlf::Unset,
        },
        Err(_) => AutoCrlf::Unset,
    }
}

/// Parse the repo-root `.gitattributes` and return the effective
/// `text`/`eol` mentions for `path`. A missing or unreadable file means no
/// attributes (all conversion then follows `core.autocrlf`). Later matching
/// rules override earlier ones (git's "last matching line wins"); within a
/// line a repeated attribute token is a parse quirk — the first mention
/// sticks.
fn attrs_for(repo: &Repository, path: &str) -> EolAttrs {
    let Some(workdir) = repo.workdir() else {
        return EolAttrs::default();
    };
    let Ok(bytes) = std::fs::read(workdir.join(".gitattributes")) else {
        return EolAttrs::default();
    };
    let text = String::from_utf8_lossy(&bytes);
    let mut attrs = EolAttrs::default();
    let mut any = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut tokens = line.split_whitespace();
        let Some(pattern) = tokens.next() else {
            continue;
        };
        // strip one level of double quotes around a quoted pattern
        let pattern = pattern.trim_matches('"');
        if !attr_pattern_matches(pattern, path) {
            continue;
        }
        for token in tokens.by_ref() {
            match token {
                "text" => {
                    attrs.text = TextAttr::True;
                    any = true;
                }
                "-text" | "binary" => {
                    // `binary` is git's builtin macro = `-text -diff ...`;
                    // only the text mention matters here.
                    attrs.text = TextAttr::False;
                    any = true;
                }
                "text=auto" => {
                    attrs.text = TextAttr::Auto;
                    any = true;
                }
                "!text" => {
                    // back to unspecified (state stack): drop any earlier read
                    attrs.text = TextAttr::Unset;
                    any = true;
                }
                "eol=lf" => {
                    attrs.eol = Some(EolAttr::Lf);
                    any = true;
                }
                "eol=crlf" => {
                    attrs.eol = Some(EolAttr::Crlf);
                    any = true;
                }
                "!eol" => {
                    attrs.eol = None;
                }
                _ => {}
            }
        }
    }
    if !any {
        return EolAttrs::default();
    }
    attrs
}

/// gitignore-class pattern match (the dialect the FE `attributesModel.ts`
/// uses): a pattern with an inner `/` anchors at the repo root, a flat
/// pattern matches the basename at any depth, `**` spans slashes, `*` and
/// `?` do not, `[...]` is a character class. This is deliberately looser
/// than full git wildmatch (no escaping, no negation) — enough for the
/// `*.ext` / `dir/` / exact-name rules repos actually pin line endings
/// with.
fn attr_pattern_matches(pattern: &str, path: &str) -> bool {
    let path = path.replace('\\', "/");
    if pattern.is_empty() || path.is_empty() {
        return false;
    }
    let dir_only = pattern.ends_with('/');
    let body = pattern.trim_end_matches('/').trim_start_matches('/');
    if body.is_empty() {
        return false;
    }
    if body.contains('/') {
        // anchored: the pattern matches the path (or, for `dir/` rules,
        // everything below the root directory it names).
        let target = path.as_bytes();
        return if dir_only {
            match_anchored_dir(body.as_bytes(), target)
        } else {
            glob_match(body.as_bytes(), target)
        };
    }
    if dir_only {
        // a flat `dir/` rule: everything below a directory of that name
        return match_flat_dir(body.as_bytes(), path.as_bytes());
    }
    let base = path.rsplit('/').next().unwrap_or(&path);
    glob_match(body.as_bytes(), base.as_bytes())
}

/// Anchored `dir/` rule: a root-level directory prefix matches.
fn match_anchored_dir(pat: &[u8], input: &[u8]) -> bool {
    input
        .iter()
        .enumerate()
        .any(|(idx, &b)| b == b'/' && glob_match(pat, &input[..idx]))
}

/// Flat `dir/` rule: a directory of that name at any depth matches.
fn match_flat_dir(pat: &[u8], input: &[u8]) -> bool {
    let mut start = 0usize;
    for (idx, &b) in input.iter().enumerate() {
        if b == b'/' {
            if glob_match(pat, &input[start..idx]) {
                return true;
            }
            start = idx + 1;
        }
    }
    false
}

/// One byte against a character-class body, with `a-z` style ranges.
fn class_contains(members: &[u8], byte: u8) -> bool {
    let mut idx = 0;
    while idx < members.len() {
        // a range is `x-y` where `-` is not the first or last member
        if idx + 2 < members.len() && members[idx + 1] == b'-' {
            if byte >= members[idx] && byte <= members[idx + 2] {
                return true;
            }
            idx += 3;
        } else {
            if members[idx] == byte {
                return true;
            }
            idx += 1;
        }
    }
    false
}

/// Glob match over bytes: `**` spans slashes, `*`/`?` do not, `[...]` (and
/// negated `[^...]`) is a character class, everything else is literal.
/// Pattern sizes here are a few dozen bytes; naive backtracking is fine.
fn glob_match(pat: &[u8], input: &[u8]) -> bool {
    if pat.is_empty() {
        return input.is_empty();
    }
    match pat[0] {
        b'*' => {
            if pat.len() >= 2 && pat[1] == b'*' {
                // `**`: any run (maybe empty), including across slashes
                for skip in 0..=input.len() {
                    if glob_match(&pat[2..], &input[skip..]) {
                        return true;
                    }
                }
                return false;
            }
            // `*`: any run of non-slash characters
            let run_end = input.iter().position(|&b| b == b'/').unwrap_or(input.len());
            for skip in 0..=run_end {
                if glob_match(&pat[1..], &input[skip..]) {
                    return true;
                }
            }
            false
        }
        b'?' => !input.is_empty() && input[0] != b'/' && glob_match(&pat[1..], &input[1..]),
        b'[' => {
            let Some(close) = pat[1..].iter().position(|&b| b == b']').map(|p| p + 1) else {
                // unterminated class: treat `[` as a literal
                return !input.is_empty() && input[0] == b'[' && glob_match(&pat[1..], &input[1..]);
            };
            let class = &pat[1..close];
            let negated = class.first() == Some(&b'^');
            let members = if negated { &class[1..] } else { class };
            let Some(&first) = input.first() else {
                return false;
            };
            if members.is_empty() {
                return false; // empty classes match nothing
            }
            let hit = class_contains(members, first) != negated;
            hit && first != b'/' && glob_match(&pat[close + 1..], &input[1..])
        }
        literal => !input.is_empty() && input[0] == literal && glob_match(&pat[1..], &input[1..]),
    }
}

/// True when a blob write to `path` should end up CRLF (checkout/smudge
/// direction). `eol=crlf` + text handling forces it, `core.autocrlf=true`
/// enables it, everything else keeps the blob bytes verbatim.
fn converts_to_workdir(attrs: &EolAttrs, autocrlf: AutoCrlf) -> bool {
    if attrs.is_binary_attr() {
        return false;
    }
    if attrs.eol == Some(EolAttr::Lf) {
        // git: an explicit `eol=lf` wins over autocrlf
        return false;
    }
    if attrs.eol == Some(EolAttr::Crlf) && attrs.text != TextAttr::Unset {
        return true;
    }
    // plain `text`/`text=auto` follow autocrlf here (git's native-eol rule
    // is host-dependent — see module note); an unset `eol` falls through.
    autocrlf == AutoCrlf::True
}

/// True when workdir content written to the index for `path` should be
/// LF-normalized (check-in/clean direction). Any live `text` attribute
/// normalizes (git parity); without attributes only an explicit
/// `autocrlf=true|input` does.
fn converts_to_index(attrs: &EolAttrs, autocrlf: AutoCrlf) -> bool {
    if attrs.is_binary_attr() {
        return false;
    }
    matches!(attrs.text, TextAttr::True | TextAttr::Auto)
        || matches!(autocrlf, AutoCrlf::True | AutoCrlf::Input)
}

/// git's binary sniff: NUL anywhere in the first 8000 bytes.
fn looks_binary(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    window.contains(&0)
}

/// Blob bytes as they belong in the WORKDIR for `path` under the repo's
/// autocrlf/attributes configuration — what `git checkout` would write
/// (smudge). Infallible: config/attribute read failures degrade to "no
/// conversion".
pub(crate) fn convert_to_workdir(repo: &Repository, path: &str, bytes: &[u8]) -> Vec<u8> {
    if bytes.is_empty() || looks_binary(bytes) {
        return bytes.to_vec();
    }
    let attrs = attrs_for(repo, path);
    if !converts_to_workdir(&attrs, autocrlf(repo)) {
        return bytes.to_vec();
    }
    // git's safe-autocrlf rule: content that already carries any CR is
    // left alone (no bare-LF rewrite of mixed files).
    if bytes.contains(&b'\r') {
        return bytes.to_vec();
    }
    lf_to_crlf(bytes)
}

/// Workdir bytes as they belong in the INDEX for `path` — what `git add`
/// stores (clean). Infallible like [`convert_to_workdir`].
pub(crate) fn convert_to_index(repo: &Repository, path: &str, bytes: &[u8]) -> Vec<u8> {
    if bytes.is_empty() || looks_binary(bytes) {
        return bytes.to_vec();
    }
    let attrs = attrs_for(repo, path);
    if !converts_to_index(&attrs, autocrlf(repo)) {
        return bytes.to_vec();
    }
    crlf_to_lf(bytes)
}

/// Line-ending style a hunk-discard surgery should write back for `path`:
/// true when the path converts on checkout AND the existing workdir file
/// already uses CRLF (so restored/kept patch lines — which come out of the
/// odb-normalized diff as LF — must be re-smudged to keep the file's
/// style). The existing-file probe is git's own heuristic flavor: a
/// converted path whose workdir file is LF stays LF.
pub(crate) fn restore_lines_as_crlf(repo: &Repository, path: &str, workdir_bytes: &[u8]) -> bool {
    if workdir_bytes.is_empty() || !workdir_bytes.windows(2).any(|w| w == b"\r\n") {
        return false;
    }
    let attrs = attrs_for(repo, path);
    converts_to_workdir(&attrs, autocrlf(repo)) && !looks_binary(workdir_bytes)
}

/// Re-smudge one diff line (odb-normalized, LF) to the workdir's CRLF
/// style. Lines without a trailing LF (final line, `\ No newline`) pass
/// through unchanged.
pub(crate) fn smudge_line(line: &[u8], crlf: bool) -> Vec<u8> {
    if !crlf {
        return line.to_vec();
    }
    lf_to_crlf(line)
}

/// Convert every bare LF to CRLF. Idempotent: an LF already preceded by a
/// CR stays single-ended (mixed/committed-CRLF content passes through with
/// only its bare LFs converted).
fn lf_to_crlf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + bytes.len() / 8);
    let mut prev_cr = false;
    for &b in bytes {
        if b == b'\n' && !prev_cr {
            out.push(b'\r');
        }
        out.push(b);
        prev_cr = b == b'\r';
    }
    out
}

/// Strip the CR from every CRLF pair (git's clean conversion; lone CRs are
/// preserved, matching libgit2's `git_str_crlf_to_git`).
fn crlf_to_lf(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut rest = bytes;
    while let Some(pos) = rest.iter().position(|&b| b == b'\r') {
        out.extend_from_slice(&rest[..pos]);
        if rest.get(pos + 1) == Some(&b'\n') {
            rest = &rest[pos + 1..]; // drop the CR, keep the LF
        } else {
            out.push(b'\r');
            rest = &rest[pos + 1..];
        }
    }
    out.extend_from_slice(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- attribute pattern matching ---------------------------------------

    #[test]
    fn star_patterns_match_basenames_at_any_depth() {
        assert!(attr_pattern_matches("*.txt", "a.txt"));
        assert!(attr_pattern_matches("*.txt", "src/deep/a.txt"));
        assert!(!attr_pattern_matches("*.txt", "a.md"));
        assert!(!attr_pattern_matches("*.txt", "a.txt.bak"));
    }

    #[test]
    fn anchored_patterns_match_paths_and_directories() {
        assert!(attr_pattern_matches("docs/*.md", "docs/readme.md"));
        assert!(!attr_pattern_matches("docs/*.md", "other/readme.md"));
        assert!(attr_pattern_matches("bin/", "bin/tool.exe"));
        assert!(attr_pattern_matches("bin/", "a/bin/tool.exe"));
        assert!(!attr_pattern_matches("bin/", "binary.txt"));
        assert!(!attr_pattern_matches("bin/", "bin")); // a file, not below it
        assert!(attr_pattern_matches("build/**/*.log", "build/x/y/e.log"));
        assert!(attr_pattern_matches("**/*.rb", "a/b/c.rb"));
    }

    #[test]
    fn question_mark_and_char_class_match_single_chars() {
        assert!(attr_pattern_matches("?.txt", "a.txt"));
        assert!(!attr_pattern_matches("?.txt", "ab.txt"));
        assert!(attr_pattern_matches("[ab].txt", "b.txt"));
        assert!(!attr_pattern_matches("[ab].txt", "c.txt"));
        assert!(attr_pattern_matches("[a-c].txt", "b.txt"));
        assert!(!attr_pattern_matches("[^a].txt", "a.txt"));
    }

    // -- conversion decisions ----------------------------------------------

    #[test]
    fn workdir_conversion_follows_autocrlf_and_attributes() {
        let none = EolAttrs::default();
        assert!(converts_to_workdir(&none, AutoCrlf::True));
        assert!(!converts_to_workdir(&none, AutoCrlf::Input)); // input never smudges
        assert!(!converts_to_workdir(&none, AutoCrlf::False));
        assert!(!converts_to_workdir(&none, AutoCrlf::Unset));

        // -text / the binary macro suppress; eol=lf overrides autocrlf
        let off = EolAttrs {
            text: TextAttr::False,
            eol: None,
        };
        assert!(!converts_to_workdir(&off, AutoCrlf::True));
        let lf = EolAttrs {
            text: TextAttr::True,
            eol: Some(EolAttr::Lf),
        };
        assert!(!converts_to_workdir(&lf, AutoCrlf::True));
        // eol=crlf forces conversion when text handling is on
        let crlf = EolAttrs {
            text: TextAttr::True,
            eol: Some(EolAttr::Crlf),
        };
        assert!(converts_to_workdir(&crlf, AutoCrlf::False));
        // eol without any text mention is inert (git: eol needs text on)
        let eol_only = EolAttrs {
            text: TextAttr::Unset,
            eol: Some(EolAttr::Crlf),
        };
        assert!(!converts_to_workdir(&eol_only, AutoCrlf::False));
        assert!(converts_to_workdir(&eol_only, AutoCrlf::True));
    }

    #[test]
    fn index_conversion_follows_autocrlf_and_attributes() {
        let none = EolAttrs::default();
        assert!(converts_to_index(&none, AutoCrlf::True));
        assert!(converts_to_index(&none, AutoCrlf::Input));
        assert!(!converts_to_index(&none, AutoCrlf::False));
        assert!(!converts_to_index(&none, AutoCrlf::Unset));
        // text=auto normalizes on check-in even without autocrlf (git parity)
        let auto = EolAttrs {
            text: TextAttr::Auto,
            eol: None,
        };
        assert!(converts_to_index(&auto, AutoCrlf::False));
        let off = EolAttrs {
            text: TextAttr::False,
            eol: None,
        };
        assert!(!converts_to_index(&off, AutoCrlf::True));
    }

    // -- byte converters -----------------------------------------------------

    #[test]
    fn lf_to_crlf_converts_bare_lfs_only() {
        assert_eq!(lf_to_crlf(b"a\nb\n"), b"a\r\nb\r\n");
        assert_eq!(lf_to_crlf(b"a"), b"a");
        assert_eq!(lf_to_crlf(b"no newline\nend"), b"no newline\r\nend");
    }

    #[test]
    fn crlf_to_lf_strips_pairs_and_keeps_lone_crs() {
        assert_eq!(crlf_to_lf(b"a\r\nb\r\n"), b"a\nb\n");
        assert_eq!(crlf_to_lf(b"mac\ronly\n"), b"mac\ronly\n");
        assert_eq!(crlf_to_lf(b"a\r\nb"), b"a\nb");
        assert_eq!(crlf_to_lf(b""), b"");
    }

    #[test]
    fn smudge_line_only_touches_when_asked() {
        assert_eq!(smudge_line(b"x\n", false), b"x\n");
        assert_eq!(smudge_line(b"x\n", true), b"x\r\n");
        // final line without a newline passes through untouched
        assert_eq!(smudge_line(b"x", true), b"x");
        // an already-CRLF line stays single-ended
        assert_eq!(smudge_line(b"x\r\n", true), b"x\r\n");
    }

    // -- attrs file parsing --------------------------------------------------

    #[test]
    fn attrs_parse_reads_the_last_matching_rule() {
        let dir = std::env::temp_dir().join(format!("mygitui-eol-attrs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let repo = git2::Repository::init(&dir).expect("init");
        std::fs::write(
            dir.join(".gitattributes"),
            "# pinned\r\n*.txt text eol=crlf\r\nspecial.txt -text\r\n",
        )
        .expect("write attrs");

        assert_eq!(
            attrs_for(&repo, "a.txt"),
            EolAttrs {
                text: TextAttr::True,
                eol: Some(EolAttr::Crlf),
            }
        );
        // the later rule overrides both mentions
        assert_eq!(attrs_for(&repo, "special.txt").text, TextAttr::False);
        // an unrelated path carries no mentions
        assert_eq!(attrs_for(&repo, "other.md"), EolAttrs::default());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
