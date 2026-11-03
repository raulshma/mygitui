//! AI feature prompt builders (ported from the old `features.ts` — the
//! strings are the spec).
//!
//! Prompt builders are PURE (no IO) and deterministic: they render git data
//! into prompts with a hard input cap ([`cap_diff`] — larger inputs are
//! budgeted per file by [`super::budget`] so every changed file stays
//! represented, with explicit truncation/omission notes) and shared
//! guardrails (never emit secrets, never emit placeholders).

use super::budget::MAX_DIFF_CHARS;
use super::{AiError, AiFeature};
use crate::engine::types::FileDiff;

/// Guardrails appended to every system prompt.
pub const GUARDRAILS: &str = "Never include secrets, API keys, tokens, passwords or credentials in your output, even if they appear in the provided data. Never emit placeholders such as \"TODO\", \"FIXME\", \"TBD\", \"...\" or \"<placeholder>\" — always produce the finished text. Base your answer strictly on the provided data; do not invent changes that are not shown.";

/// Shared subject/body output contract for message-style prompts.
const MESSAGE_FORMAT: &str = "Output format — exactly this shape, no code fences, no extra commentary:\n<subject line>\n\n<optional body paragraphs>\nThe first line is the subject; everything after the first blank line is the body.";

/// One commit for the PR title/body context (short sha + subject).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRef {
    pub sha: String,
    pub summary: String,
}

/// Everything a prompt builder needs, as plain strings. Only the fields a
/// feature consumes must be set; everything else defaults to empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PromptContext {
    /// Budgeted staged-diff text (commit-message, review-staged) or the
    /// work-in-progress summary (stash-message).
    pub staged_diff: Option<String>,
    /// Per-file add/del stats (pr-title-body).
    pub diff_summary: Option<String>,
    /// Branch commits (pr-title-body).
    pub commits: Vec<CommitRef>,
    /// explain-hunk: file path.
    pub path: Option<String>,
    /// explain-hunk: the hunk text to explain.
    pub hunk: Option<String>,
    /// branch-name: what the branch is for, free-form.
    pub description: Option<String>,
    /// branch-name flavor: recent commit subjects (up to five are used).
    pub recent_subjects: Vec<String>,
    /// branch-name flavor: the branch we branch from, when known.
    pub base_branch: Option<String>,
}

/// Truncates `text` to `max` characters, appending an explicit note. The
/// cut backs up to the last complete line so the model never sees a half
/// hunk.
pub fn cap_diff(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_string();
    }
    let capped: String = text.chars().take(max).collect();
    let shown = match capped.rfind('\n') {
        Some(position) if position > 0 => capped[..position].to_string(),
        _ => capped,
    };
    let shown_len = shown.chars().count();
    format!("{shown}\n\n[diff truncated: showing first {shown_len} of {total} characters]")
}

/// Builds the (system, user) prompts for `feature` from `context`.
/// Missing required context is an [`AiError::InvalidInput`] — the service
/// layer surfaces it before any network call.
pub fn prompts(feature: AiFeature, context: &PromptContext) -> Result<(String, String), AiError> {
    match feature {
        AiFeature::CommitMessage => {
            let diff = require_diff(context, "nothing is staged to write a commit message for")?;
            Ok(commit_message_prompt(diff))
        }
        AiFeature::PrTitleBody => Ok(pr_title_body_prompt(
            &context.commits,
            context.diff_summary.as_deref(),
        )),
        AiFeature::ExplainHunk => {
            let (Some(path), Some(hunk)) = (context.path.as_deref(), context.hunk.as_deref())
            else {
                return Err(AiError::InvalidInput(
                    "explain-hunk needs a file path and hunk text".into(),
                ));
            };
            Ok(explain_hunk_prompt(path, hunk))
        }
        AiFeature::ReviewStaged => {
            let diff = require_diff(context, "nothing is staged to review")?;
            Ok(review_staged_prompt(diff))
        }
        AiFeature::StashMessage => {
            let diff = require_diff(context, "no work-in-progress changes to describe")?;
            Ok(stash_message_prompt(&cap_diff(diff, 2_000)))
        }
        AiFeature::BranchName => {
            let Some(description) = context
                .description
                .as_deref()
                .map(str::trim)
                .filter(|d| !d.is_empty())
            else {
                return Err(AiError::InvalidInput(
                    "branch-name needs a description".into(),
                ));
            };
            Ok(branch_name_prompt(&branch_name_description(
                description,
                &context.recent_subjects,
                context.base_branch.as_deref(),
            )))
        }
    }
}

fn require_diff<'a>(context: &'a PromptContext, missing: &str) -> Result<&'a str, AiError> {
    context
        .staged_diff
        .as_deref()
        .map(str::trim)
        .filter(|diff| !diff.is_empty())
        .ok_or_else(|| AiError::InvalidInput(missing.into()))
}

/// System + user prompt for a commit message from the staged diff
/// (Conventional Commits: type prefix, ≤50-char subject, imperative mood).
pub fn commit_message_prompt(staged_diff: &str) -> (String, String) {
    let system = [
        "You write git commit messages for a desktop Git client.",
        "The input starts with a stat summary of ALL changed files; the diff content after it may be budgeted per file, so trust the summary when the change spans many files.",
        "Follow the Conventional Commits specification: the subject starts with a type such as feat, fix, docs, style, refactor, perf, test, build, ci or chore, optionally with a scope like `feat(ui):`.",
        "Keep the subject at most 50 characters, in imperative mood, with no trailing period.",
        "The body explains WHY the change was made, not WHAT changed mechanically (the diff already shows what) — omit the body entirely when there is nothing meaningful to add.",
        "Wrap body lines at about 72 characters.",
        GUARDRAILS,
    ]
    .join("\n\n");
    let prompt = [
        "Write a commit message for the following staged changes.",
        "",
        "Staged diff:",
        "```",
        &cap_diff(staged_diff, MAX_DIFF_CHARS),
        "```",
        "",
        MESSAGE_FORMAT,
    ]
    .join("\n");
    (system, prompt)
}

/// Pull request title + body prompt from the branch's commits + diff stats.
pub fn pr_title_body_prompt(commits: &[CommitRef], diff_summary: Option<&str>) -> (String, String) {
    let system = [
        "You write pull request titles and descriptions for a desktop Git client.",
        "The title is a single line (at most 72 characters, imperative mood, no trailing period).",
        "The body summarizes the motivation and the main changes as short bullet points.",
        GUARDRAILS,
    ]
    .join("\n\n");
    let commit_lines = commits
        .iter()
        .map(|commit| {
            format!(
                "- {} {}",
                &commit.sha[..commit.sha.chars().count().min(7)],
                commit.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = [
        "Write a pull request title and description for this branch.",
        "",
        "Commits (oldest intent, newest last is not guaranteed):",
        &if commit_lines.is_empty() {
            "(no commits listed)".to_string()
        } else {
            commit_lines
        },
        "",
        "Changed files:",
        diff_summary.unwrap_or("(no file stats)"),
        "",
        MESSAGE_FORMAT,
    ]
    .join("\n");
    (system, prompt)
}

/// Prompt explaining a single diff hunk in a file.
pub fn explain_hunk_prompt(path: &str, hunk_text: &str) -> (String, String) {
    let system = [
        "You explain code diffs to developers using a desktop Git client.",
        "Explain concisely what the change does and any risk it carries (2–5 sentences).",
        GUARDRAILS,
    ]
    .join("\n\n");
    let prompt = [
        &format!("Explain this change in `{path}`:"),
        "",
        "```diff",
        &cap_diff(hunk_text, MAX_DIFF_CHARS),
        "```",
    ]
    .join("\n");
    (system, prompt)
}

/// Prompt reviewing the full staged diff for problems.
pub fn review_staged_prompt(staged_diff: &str) -> (String, String) {
    let system = [
        "You are a careful code reviewer inside a desktop Git client.",
        "The input starts with a stat summary of ALL changed files; the diff content after it may be budgeted per file. Review what is shown and note when the summary suggests more.",
        "Review the staged changes: correctness risks, edge cases, security issues and style problems. Be concrete and reference file paths. When everything looks fine, say so briefly.",
        GUARDRAILS,
    ]
    .join("\n\n");
    let prompt = [
        "Review the following staged changes.",
        "",
        "Staged diff:",
        "```",
        &cap_diff(staged_diff, MAX_DIFF_CHARS),
        "```",
    ]
    .join("\n");
    (system, prompt)
}

/// Prompt for a stash message from a short change summary.
pub fn stash_message_prompt(summary: &str) -> (String, String) {
    let system = [
        "You write short git stash messages for a desktop Git client.",
        "One line, at most 60 characters, describing what is being stashed and why it is parked.",
        GUARDRAILS,
    ]
    .join("\n\n");
    let prompt = [
        "Write a stash message for these work-in-progress changes:",
        "",
        summary,
        "",
        MESSAGE_FORMAT,
    ]
    .join("\n");
    (system, prompt)
}

/// How many recent subjects are folded into the branch-name description.
pub const MAX_SUBJECTS: usize = 5;

/// Renders the branch-name context into the single description string the
/// prompt expects: the user's description first, then up to
/// [`MAX_SUBJECTS`] recent subjects, then the base branch. Empty parts are
/// skipped (ported from the old `branchName.ts`).
pub fn branch_name_description(
    description: &str,
    recent_subjects: &[String],
    base_branch: Option<&str>,
) -> String {
    let collapse = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut parts: Vec<String> = Vec::new();
    let description = collapse(description);
    if !description.is_empty() {
        parts.push(format!("What it is for: {description}"));
    }
    let subjects = recent_subjects
        .iter()
        .map(|subject| collapse(subject))
        .filter(|subject| !subject.is_empty())
        .take(MAX_SUBJECTS)
        .collect::<Vec<_>>();
    if !subjects.is_empty() {
        parts.push(format!("Recent commit subjects: {}", subjects.join("; ")));
    }
    let base = base_branch.map(str::trim).filter(|base| !base.is_empty());
    if let Some(base) = base {
        parts.push(format!("Branches from: {base}"));
    }
    parts.join("\n")
}

/// Prompt for a kebab-case branch name from a free-form description.
pub fn branch_name_prompt(description: &str) -> (String, String) {
    let system = [
        "You suggest git branch names for a desktop Git client.",
        "Answer with ONE line: a kebab-case branch name (lowercase words separated by hyphens, no spaces, no slashes, no type prefix unless the description asks for one).",
        GUARDRAILS,
    ]
    .join("\n\n");
    let prompt =
        format!("Suggest a branch name for: {description}\n\nAnswer with the branch name only.");
    (system, prompt)
}

/// Parses a model answer in the subject/blank/body format. Tolerates code
/// fences, leading "Subject:"-style labels and stray blank lines. Errors
/// when no usable subject exists.
pub fn parse_commit_message(text: &str) -> Result<(String, String), AiError> {
    let mut clean = text.trim().to_string();
    // Tolerate a single fenced block around the whole answer: ```lang\n…
    // ``` (the old regex accepted an optional language tag).
    if clean.starts_with("```") && clean.ends_with("```") && clean.len() > 3 {
        if let Some(first_newline) = clean.find('\n') {
            let inner = &clean[first_newline + 1..clean.len() - 3];
            let inner = inner.strip_suffix('\n').unwrap_or(inner);
            clean = inner.trim().to_string();
        }
    }
    let mut lines = clean.split('\n');
    let subject_raw = lines.next().unwrap_or("").trim().to_string();
    let lower = subject_raw.to_ascii_lowercase();
    let subject = if lower.starts_with("subject:") {
        subject_raw["subject:".len()..].trim_start().to_string()
    } else if lower.starts_with("title:") {
        subject_raw["title:".len()..].trim_start().to_string()
    } else {
        subject_raw
    };
    let body = lines.collect::<Vec<_>>().join("\n");
    let body = body.trim_start_matches('\n').trim_end().to_string();
    if subject.is_empty() {
        return Err(AiError::BadResponse(
            "AI returned an empty commit subject".into(),
        ));
    }
    Ok((subject, body))
}

/// Extracts the kebab-case branch name from a model answer (last non-empty
/// line, sanitized to `[a-z0-9-]`, trailing hyphens stripped). Errors when
/// nothing usable remains.
pub fn parse_branch_name(text: &str) -> Result<String, AiError> {
    // The answer's LAST non-empty line carries the name (models often add
    // a sentence first).
    let mut last_line = "";
    for line in text.trim().split('\n').map(str::trim) {
        if !line.is_empty() {
            last_line = line;
        }
    }
    let raw = last_line
        .trim_start_matches(['-', '`', '*', ' '])
        .replace(['`', '*'], "")
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    if raw.is_empty() {
        return Err(AiError::BadResponse(
            "AI returned no usable branch name".into(),
        ));
    }
    Ok(raw)
}

// ---------------------------------------------------------------------------
// Git data → prompt text (pure renderers over FileDiff)
// ---------------------------------------------------------------------------

/// Renders one [`FileDiff`] as unified-diff-style text.
pub fn format_file_diff(file: &FileDiff) -> String {
    if file.binary || file.is_image {
        return format!(
            "Binary file {} changed (+{}/-{})",
            file.path, file.additions, file.deletions
        );
    }
    let old = file
        .old_path
        .as_deref()
        .filter(|old| !old.is_empty() && *old != file.path)
        .unwrap_or(file.path.as_str());
    let mut out: Vec<String> = vec![format!("--- a/{old}"), format!("+++ b/{}", file.path)];
    for hunk in &file.hunks {
        out.push(format!("@@ -{} +{} @@", hunk.old_start, hunk.new_start));
        for line in &hunk.lines {
            let marker = if line.origin == '=' { ' ' } else { line.origin };
            out.push(format!("{marker}{}", line.text));
        }
    }
    out.join("\n")
}

/// Renders the staged diff (list of [`FileDiff`]) as prompt text.
pub fn staged_diff_text(files: &[FileDiff]) -> String {
    files
        .iter()
        .map(format_file_diff)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Renders per-file add/del stats (used by the PR body prompt).
pub fn diff_summary_text(files: &[FileDiff]) -> String {
    files
        .iter()
        .map(|file| format!("- {} (+{}/-{})", file.path, file.additions, file.deletions))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::{DiffHunk, DiffLine};

    fn line(origin: char, text: &str) -> DiffLine {
        DiffLine {
            old_no: None,
            new_no: None,
            origin,
            text: text.into(),
            highlights: vec![],
        }
    }

    fn file(path: &str, old_path: Option<&str>, binary: bool, hunks: Vec<DiffHunk>) -> FileDiff {
        FileDiff {
            path: path.into(),
            old_path: old_path.map(str::to_string),
            binary,
            is_image: false,
            additions: hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.origin == '+')
                .count() as u32,
            deletions: hunks
                .iter()
                .flat_map(|h| h.lines.iter())
                .filter(|l| l.origin == '-')
                .count() as u32,
            hunks,
        }
    }

    fn hunk(lines: Vec<DiffLine>) -> DiffHunk {
        DiffHunk {
            old_start: 1,
            new_start: 1,
            lines,
        }
    }

    // -- cap_diff ------------------------------------------------------------

    #[test]
    fn cap_diff_passes_short_text_through() {
        assert_eq!(cap_diff("short", 100), "short");
    }

    #[test]
    fn cap_diff_truncates_on_line_boundary_with_note() {
        let text = format!("{}\n{}\n{}", "a".repeat(60), "b".repeat(60), "c".repeat(60));
        let capped = cap_diff(&text, 70);
        assert!(
            capped.starts_with(&"a".repeat(60)),
            "cut back to the last full line"
        );
        assert!(
            capped.contains("[diff truncated: showing first 60 of "),
            "note: {capped}"
        );
        assert!(!capped.contains("bbb"), "the second line is gone");
    }

    #[test]
    fn cap_diff_handles_multibyte_characters() {
        let text = "é".repeat(150);
        let capped = cap_diff(&text, 100);
        assert!(
            capped.contains("showing first 100 of 150 characters"),
            "{capped}"
        );
        // No panic mid-character: the shown prefix is valid UTF-8 with the
        // exact character count.
        assert_eq!(
            capped.chars().count(),
            100 + "\n\n[diff truncated: showing first 100 of 150 characters]"
                .chars()
                .count()
        );
    }

    // -- builders ------------------------------------------------------------

    #[test]
    fn commit_message_prompt_is_conventional_and_carries_diff() {
        let (system, prompt) = commit_message_prompt("diff --git a/x b/x");
        assert!(system.contains("Conventional Commits"));
        assert!(system.contains("50 characters"));
        assert!(system.contains("Never include secrets"));
        assert!(prompt.contains("Staged diff:"));
        assert!(prompt.contains("diff --git a/x b/x"));
        assert!(prompt.contains("<subject line>"));
    }

    #[test]
    fn pr_prompt_lists_commits_and_stats_with_fallbacks() {
        let commits = vec![
            CommitRef {
                sha: "abcdef1234567890".into(),
                summary: "first".into(),
            },
            CommitRef {
                sha: "0fedcba9876543".into(),
                summary: "second".into(),
            },
        ];
        let (system, prompt) = pr_title_body_prompt(&commits, Some("- src/main.rs (+10/-2)"));
        assert!(system.contains("pull request"));
        assert!(
            prompt.contains("- abcdef1 first"),
            "sha shortened to 7: {prompt}"
        );
        assert!(prompt.contains("- 0fedcb second") || prompt.contains("- 0fedcba second"));
        assert!(prompt.contains("- src/main.rs (+10/-2)"));

        let (_, empty) = pr_title_body_prompt(&[], None);
        assert!(empty.contains("(no commits listed)"));
        assert!(empty.contains("(no file stats)"));
    }

    #[test]
    fn explain_hunk_prompt_names_path_and_wraps_hunk() {
        let (system, prompt) = explain_hunk_prompt("src/app.rs", "@@ -1 +1 @@\n-old\n+new");
        assert!(system.contains("explain"));
        assert!(prompt.contains("Explain this change in `src/app.rs`:"));
        assert!(prompt.contains("```diff"));
        assert!(prompt.contains("+new"));
    }

    #[test]
    fn stash_prompt_is_one_line_contract() {
        let (_, prompt) = stash_message_prompt("wip: login flow");
        assert!(prompt.contains("stash message"));
        assert!(prompt.contains("wip: login flow"));
    }

    #[test]
    fn branch_name_description_folds_context_and_skips_empties() {
        assert_eq!(
            branch_name_description("fix  the login crash", &[], None),
            "What it is for: fix the login crash"
        );
        assert_eq!(
            branch_name_description(
                "add tests",
                &["first subject".into(), "".into(), "second subject".into()],
                Some("main")
            ),
            "What it is for: add tests\nRecent commit subjects: first subject; second subject\nBranches from: main"
        );
        assert_eq!(
            branch_name_description("   ", &[], None),
            "",
            "blank description yields nothing"
        );
    }

    #[test]
    fn branch_name_prompt_asks_for_one_line() {
        let (system, prompt) = branch_name_prompt("What it is for: add tests");
        assert!(system.contains("kebab-case"));
        assert!(prompt.starts_with("Suggest a branch name for:"));
        assert!(prompt.ends_with("Answer with the branch name only."));
    }

    // -- prompts() dispatch --------------------------------------------------

    #[test]
    fn prompts_dispatches_all_six_features() {
        let ctx = PromptContext {
            staged_diff: Some("diff".into()),
            diff_summary: Some("- a (+1/-1)".into()),
            commits: vec![CommitRef {
                sha: "abcdef1".into(),
                summary: "s".into(),
            }],
            path: Some("p".into()),
            hunk: Some("@@".into()),
            description: Some("desc".into()),
            recent_subjects: vec![],
            base_branch: None,
        };
        for feature in [
            AiFeature::CommitMessage,
            AiFeature::PrTitleBody,
            AiFeature::ExplainHunk,
            AiFeature::ReviewStaged,
            AiFeature::StashMessage,
            AiFeature::BranchName,
        ] {
            let (system, prompt) =
                prompts(feature, &ctx).unwrap_or_else(|e| panic!("{feature:?}: {e}"));
            assert!(!system.is_empty() && !prompt.is_empty());
            assert!(
                system.contains("Never include secrets"),
                "{feature:?} misses guardrails"
            );
        }
    }

    #[test]
    fn prompts_reject_missing_required_context_before_network() {
        let empty = PromptContext::default();
        let err = prompts(AiFeature::CommitMessage, &empty).unwrap_err();
        assert_eq!(
            err.message(),
            "nothing is staged to write a commit message for"
        );
        let err = prompts(AiFeature::ReviewStaged, &empty).unwrap_err();
        assert_eq!(err.message(), "nothing is staged to review");
        let err = prompts(AiFeature::StashMessage, &empty).unwrap_err();
        assert_eq!(err.message(), "no work-in-progress changes to describe");
        let err = prompts(AiFeature::ExplainHunk, &empty).unwrap_err();
        assert_eq!(
            err.message(),
            "explain-hunk needs a file path and hunk text"
        );
        let err = prompts(AiFeature::BranchName, &empty).unwrap_err();
        assert_eq!(err.message(), "branch-name needs a description");
        // Whitespace-only diff counts as nothing.
        let ctx = PromptContext {
            staged_diff: Some("   \n".into()),
            ..Default::default()
        };
        assert!(prompts(AiFeature::CommitMessage, &ctx).is_err());
    }

    #[test]
    fn stash_message_is_capped_at_2000_chars() {
        let long = "x".repeat(5000);
        let ctx = PromptContext {
            staged_diff: Some(long),
            ..Default::default()
        };
        let (_, prompt) = prompts(AiFeature::StashMessage, &ctx).unwrap();
        assert!(prompt.contains("[diff truncated:"));
        assert!(prompt.chars().count() < 5000 + 500);
    }

    // -- answer parsers ------------------------------------------------------

    #[test]
    fn parse_commit_message_tolerates_fences_and_labels() {
        assert_eq!(
            parse_commit_message("feat(ui): add panel\n\nWhy: nicer UI").unwrap(),
            ("feat(ui): add panel".into(), "Why: nicer UI".into())
        );
        assert_eq!(
            parse_commit_message("```txt\nfix: thing\n\nbody here\n```").unwrap(),
            ("fix: thing".into(), "body here".into())
        );
        assert_eq!(
            parse_commit_message("Subject: chore: cleanup").unwrap().0,
            "chore: cleanup"
        );
        assert_eq!(parse_commit_message("Title: just a title\n").unwrap().1, "");
        // Stray blank lines after the subject do not stay in the body.
        assert_eq!(parse_commit_message("t\n\n\nbody").unwrap().1, "body");
        assert!(parse_commit_message("").is_err());
        assert!(parse_commit_message("   \n  ").is_err());
    }

    #[test]
    fn parse_branch_name_sanitizes_last_usable_line() {
        assert_eq!(
            parse_branch_name("Here you go:\n- `add-tests`").unwrap(),
            "add-tests"
        );
        assert_eq!(
            parse_branch_name("Fix Login Crash!").unwrap(),
            "fix-login-crash"
        );
        assert_eq!(parse_branch_name("  feat/add_ai  ").unwrap(), "feat-add-ai");
        assert_eq!(
            parse_branch_name("multi--hyphen$$x").unwrap(),
            "multi-hyphen-x"
        );
        assert!(parse_branch_name("```\n```").is_err());
        assert!(parse_branch_name("").is_err());
        assert!(parse_branch_name("---***---").is_err());
    }

    // -- diff renderers ------------------------------------------------------

    #[test]
    fn file_diff_renders_unified_style_with_rename_awareness() {
        let renamed = file(
            "new.rs",
            Some("old.rs"),
            false,
            vec![hunk(vec![
                line(' ', "ctx"),
                line('-', "gone"),
                line('+', "added"),
            ])],
        );
        let text = format_file_diff(&renamed);
        assert!(text.contains("--- a/old.rs"));
        assert!(text.contains("+++ b/new.rs"));
        assert!(text.contains("@@ -1 +1 @@"));
        assert!(text.contains(" ctx\n-gone\n+added"));

        // Same path: no rename header divergence.
        let same = file("a.rs", Some("a.rs"), false, vec![]);
        assert!(format_file_diff(&same).starts_with("--- a/a.rs\n+++ b/a.rs"));

        // Binary files render one stat line, no hunks.
        let binary = file("img.png", None, true, vec![]);
        assert_eq!(
            format_file_diff(&binary),
            "Binary file img.png changed (+0/-0)"
        );
    }

    #[test]
    fn summary_renderers_join_per_file_lines() {
        let files = vec![
            file("a.rs", None, false, vec![hunk(vec![line('+', "x")])]),
            file("b.bin", None, true, vec![]),
        ];
        assert_eq!(diff_summary_text(&files), "- a.rs (+1/-0)\n- b.bin (+0/-0)");
        let staged = staged_diff_text(&files);
        assert!(staged.contains("--- a/a.rs"));
        assert!(staged.contains("Binary file b.bin changed (+0/-0)"));
    }
}
