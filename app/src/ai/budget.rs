//! File-aware diff budgeting for AI prompts (ported from the old
//! `diffBudget.ts` — the constants and layout are the spec).
//!
//! The naive approach — render the whole staged diff to one string and cut
//! at [`MAX_DIFF_CHARS`] — drops every file after the cap without the model
//! ever knowing they existed, and lets a single lockfile eat the entire
//! budget. This module instead renders an always-complete stat summary
//! first, then spends the remaining character budget across files: every
//! eligible file gets a fair slice (capped per file), oversized slices end
//! with an explicit "[+N more changed lines in this file]" note,
//! generated/lockfile content is excluded (its stat line remains), and
//! files that receive no content are listed by path so the model can still
//! reason about the whole change.
//!
//! All functions are PURE (no IO) so they compose with the prompt builders
//! in [`super::prompts`].

use crate::engine::types::{DiffHunk, FileDiff};

/// Hard cap for the whole rendered text, sized for a ~128k-token LLM
/// context: ~400k chars ≈ 100k+ tokens of diff at a typical 3.5–4
/// chars/token, leaving room for the system prompt and the response.
pub const MAX_DIFF_CHARS: usize = 400_000;

/// Max chars of diff content for any single file.
pub const PER_FILE_CAP: usize = 32_000;

/// Smallest useful per-file slice (file headers + hunk header + a few lines).
pub const MIN_FILE_SHARE: usize = 400;

/// Max entries in the stat summary before it collapses to "and N more".
pub const MAX_STAT_ENTRIES: usize = 500;

/// How many omitted file paths to name explicitly before "and K more".
const MAX_OMITTED_LISTED: usize = 20;

/// Budget knobs (all three default to the module constants).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffBudget {
    /// Total char budget for the rendered text.
    pub max_chars: usize,
    /// Max content chars per file.
    pub per_file_cap: usize,
    /// Max stat-summary entries.
    pub max_stat_entries: usize,
}

impl Default for DiffBudget {
    fn default() -> Self {
        Self {
            max_chars: MAX_DIFF_CHARS,
            per_file_cap: PER_FILE_CAP,
            max_stat_entries: MAX_STAT_ENTRIES,
        }
    }
}

/// Reserve for section headers and the omitted-files note (budget-scaled).
fn overhead_reserve(max_chars: usize) -> usize {
    (max_chars / 50).clamp(300, 2_000)
}

/// Lockfiles and manifests whose content never earns a slice.
const GENERATED_BASENAMES: [&str; 11] = [
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lockb",
    "bun.lock",
    "Cargo.lock",
    "composer.lock",
    "poetry.lock",
    "Gemfile.lock",
    "go.sum",
    "Package.resolved",
];

/// Minified-source and sourcemap suffixes (the old adapter's `\.min\.…$`
/// and `\.map$` regexes).
const MINIFIED_SUFFIXES: [&str; 10] = [
    ".min.js", ".min.jsx", ".min.cjs", ".min.mjs", ".min.ts", ".min.tsx", ".min.cts", ".min.mts",
    ".min.css", ".map",
];

/// True when `path` looks generated or vendored: its content is excluded
/// from the budget (the stat summary still lists it). Ported patterns:
/// dist/build/vendor/node_modules directories, minified JS/CSS, sourcemaps.
pub fn is_generated_path(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    if GENERATED_BASENAMES.contains(&base) {
        return true;
    }
    let segments = format!("/{path}/");
    let generated_dir = ["/dist/", "/build/", "/vendor/", "/node_modules/"]
        .iter()
        .any(|marker| segments.contains(marker));
    generated_dir
        || MINIFIED_SUFFIXES
            .iter()
            .any(|suffix| path.ends_with(suffix))
}

/// One stat-summary line (mirrors [`super::prompts::diff_summary_text`]'s
/// style).
fn stat_line(file: &FileDiff) -> String {
    let kind = if file.binary || file.is_image {
        " (binary)"
    } else {
        ""
    };
    format!(
        "- {}{} (+{}/-{})",
        file.path, kind, file.additions, file.deletions
    )
}

/// Renders the complete change overview: one `path (+a/-d)` line per file.
/// Beyond `max_entries` entries collapses into an explicit "and N more
/// files" note so huge changes stay bounded but honest.
pub fn stat_summary_text(files: &[FileDiff], max_entries: usize) -> String {
    let lines: Vec<String> = files.iter().take(max_entries).map(stat_line).collect();
    let rest = files.len().saturating_sub(lines.len());
    let mut listed = vec![format!("Files changed ({}):", files.len())];
    listed.extend(lines);
    if rest > 0 {
        listed.push(format!("… and {rest} more files (not listed)"));
    }
    listed.join("\n")
}

/// Counts changed (+/-) lines across a file's hunks.
fn changed_line_count(hunks: &[DiffHunk]) -> usize {
    hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .filter(|line| line.origin == '+' || line.origin == '-')
        .count()
}

/// Renders one file's diff within `max_chars` (counted in characters):
/// unified-style headers and hunks, cut mid-hunk when the slice runs out,
/// with an explicit "[+N more changed lines in this file]" note (context
/// lines excluded from the count). The file headers are always included.
pub fn render_file_slice(file: &FileDiff, max_chars: usize) -> String {
    let old = file
        .old_path
        .as_deref()
        .filter(|old| !old.is_empty() && *old != file.path)
        .unwrap_or(file.path.as_str());
    let header = format!("--- a/{old}\n+++ b/{}\n", file.path);
    let mut out: Vec<String> = vec![header.clone()];
    let mut used = header.chars().count();
    let mut shown_changed = 0usize;
    let total_changed = changed_line_count(&file.hunks);

    'outer: for hunk in &file.hunks {
        let hunk_header = format!("@@ -{} +{} @@", hunk.old_start, hunk.new_start);
        if used + hunk_header.chars().count() + 1 > max_chars {
            break;
        }
        out.push(hunk_header.clone());
        used += hunk_header.chars().count() + 1;
        for line in &hunk.lines {
            let marker = if line.origin == '=' { ' ' } else { line.origin };
            let text = format!("{marker}{}", line.text);
            let text_len = text.chars().count();
            if used + text_len + 1 > max_chars {
                break 'outer;
            }
            out.push(text);
            used += text_len + 1;
            if line.origin == '+' || line.origin == '-' {
                shown_changed += 1;
            }
        }
    }

    let omitted = total_changed - shown_changed;
    if omitted > 0 {
        out.push(format!("[+{omitted} more changed lines in this file]"));
    }
    out.join("\n")
}

/// Renders staged files as prompt text under a hard character budget:
/// stat summary first (always complete to its cap), then per-file diff
/// content with a fair per-file slice, then an explicit list of files
/// whose content was omitted. Small inputs render essentially the full
/// diff, so nothing changes for everyday commits.
pub fn budgeted_diff_text(files: &[FileDiff], budget: &DiffBudget) -> String {
    let stats = stat_summary_text(files, budget.max_stat_entries);

    let mut sections: Vec<String> = vec![stats];
    let mut omitted: Vec<String> = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let content_budget = budget
        .max_chars
        .saturating_sub(sections[0].chars().count() + overhead_reserve(budget.max_chars));
    // Binary/image files never had diff content to begin with — they stay
    // out of both the content sections and the omitted note (their stat
    // line already says "(binary)"). Generated/lockfile files ARE listed
    // as omitted so the model never mistakes the budget for the truth.
    let eligible: Vec<&FileDiff> = files
        .iter()
        .filter(|file| !file.binary && !file.is_image)
        .collect();
    let share = if eligible.is_empty() {
        0
    } else {
        budget
            .per_file_cap
            .min((content_budget / eligible.len()).max(MIN_FILE_SHARE))
    };
    let mut spent = 0usize;
    for file in &eligible {
        if is_generated_path(&file.path) {
            omitted.push(file.path.clone());
            continue;
        }
        let remaining = content_budget.saturating_sub(spent);
        if remaining < MIN_FILE_SHARE {
            omitted.push(file.path.clone());
            continue;
        }
        let slice = render_file_slice(file, share.min(remaining));
        spent += slice.chars().count();
        parts.push(slice);
    }
    if !parts.is_empty() {
        // Header only when something is missing — small changes keep the
        // plain "stats + diff" shape.
        if parts.len() < eligible.len() {
            sections.push(format!(
                "Diff content ({} of {} files, per-file budget):",
                parts.len(),
                files.len()
            ));
        }
        sections.push(parts.join("\n\n"));
    }
    if !omitted.is_empty() {
        let named = omitted
            .iter()
            .take(MAX_OMITTED_LISTED)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let listed_count = omitted.len().min(MAX_OMITTED_LISTED);
        let rest = omitted.len() - listed_count;
        let plural = if omitted.len() == 1 { "" } else { "s" };
        let more = if rest > 0 {
            format!(", and {rest} more")
        } else {
            String::new()
        };
        sections.push(format!(
            "[Content omitted for {len} file{plural} (stats above): {named}{more}]",
            len = omitted.len()
        ));
    }
    sections.join("\n\n")
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

    fn file(path: &str, binary: bool, hunks: Vec<DiffHunk>) -> FileDiff {
        FileDiff {
            path: path.into(),
            old_path: None,
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

    fn text_file(path: &str, changed: usize) -> FileDiff {
        let mut lines = Vec::new();
        for index in 0..changed {
            lines.push(line(' ', "ctx"));
            lines.push(line('+', &format!("added line {index} with some content")));
        }
        file(path, false, vec![hunk(lines)])
    }

    #[test]
    fn generated_path_detection() {
        for path in [
            "package-lock.json",
            "web/package-lock.json",
            "Cargo.lock",
            "vendor/x/y.rs",
            "dist/app.js",
            "node_modules/left-pad/index.js",
            "app.min.js",
            "styles.min.css",
            "bundle.js.map",
        ] {
            assert!(is_generated_path(path), "{path} should be generated");
        }
        for path in [
            "src/main.rs",
            "src/minimal.js",
            "assets/logo.png",
            "docs/build.md",
        ] {
            assert!(!is_generated_path(path), "{path} should not be generated");
        }
    }

    #[test]
    fn stat_summary_is_complete_and_collapses_huge_lists() {
        let files = vec![text_file("a.rs", 2), text_file("b.rs", 2)];
        let stats = stat_summary_text(&files, 500);
        assert!(stats.contains("Files changed (2):"));
        assert!(stats.contains("- a.rs (+2/-0)"));
        assert!(stats.contains("- b.rs (+2/-0)"));
        assert!(!stats.contains("more files"));

        let many: Vec<FileDiff> = (0..7).map(|i| text_file(&format!("f{i}.rs"), 1)).collect();
        let stats = stat_summary_text(&many, 3);
        assert!(stats.contains("- f0.rs"));
        assert!(stats.contains("- f2.rs"));
        assert!(!stats.contains("- f3.rs"));
        assert!(stats.contains("… and 4 more files (not listed)"));
    }

    #[test]
    fn file_slice_cuts_mid_hunk_with_explicit_note() {
        let big = text_file("big.rs", 40);
        let slice = render_file_slice(&big, 200);
        assert!(slice.starts_with("--- a/big.rs\n+++ b/big.rs\n"));
        assert!(slice.contains("[+"));
        assert!(slice.contains("more changed lines in this file]"));
        // Fewer changed lines shown than the 40 the file carries.
        assert!(slice.matches("+added line").count() < 40);
    }

    #[test]
    fn small_inputs_render_stats_plus_full_diff() {
        let files = vec![text_file("a.rs", 2), text_file("b.rs", 2)];
        let text = budgeted_diff_text(&files, &DiffBudget::default());
        assert!(text.contains("Files changed (2):"));
        assert!(text.contains("added line 0"), "{text}");
        assert!(
            !text.contains("Content omitted"),
            "nothing is omitted: {text}"
        );
        assert!(
            !text.contains("Diff content ("),
            "no budget header for small changes"
        );
    }

    #[test]
    fn generated_files_are_listed_as_omitted_but_keep_their_stat_line() {
        let files = vec![text_file("Cargo.lock", 5), text_file("src/main.rs", 2)];
        let text = budgeted_diff_text(&files, &DiffBudget::default());
        assert!(
            text.contains("- Cargo.lock (+5/-0)"),
            "stat line stays: {text}"
        );
        assert!(text.contains("Content omitted for 1 file"));
        assert!(text.contains("Cargo.lock"));
        assert!(
            text.contains("added line 0 with some content"),
            "eligible file content stays"
        );
    }

    #[test]
    fn tiny_budgets_split_fairly_and_note_omissions() {
        let files: Vec<FileDiff> = (0..4).map(|i| text_file(&format!("f{i}.rs"), 30)).collect();
        let budget = DiffBudget {
            max_chars: 2_000,
            per_file_cap: 32_000,
            max_stat_entries: 500,
        };
        let text = budgeted_diff_text(&files, &budget);
        assert!(text.contains("Files changed (4):"));
        assert!(
            text.contains("Diff content ("),
            "budget header appears: {text}"
        );
        // Every eligible file either shows content or is named as omitted.
        for i in 0..4 {
            let path = format!("f{i}.rs");
            assert!(
                text.contains(&path),
                "{path} must appear (content or omission note): {text}"
            );
        }
    }

    #[test]
    fn binary_files_never_earn_content_or_omission_notes() {
        let files = vec![file("img.png", true, vec![]), text_file("a.rs", 1)];
        let text = budgeted_diff_text(&files, &DiffBudget::default());
        assert!(text.contains("- img.png (binary) (+0/-0)"), "{text}");
        assert!(!text.contains("Content omitted"));
        assert!(
            !text.contains("Binary file img.png"),
            "binary stat line comes from the summary"
        );
    }

    #[test]
    fn empty_input_renders_the_summary_only() {
        let text = budgeted_diff_text(&[], &DiffBudget::default());
        assert_eq!(text, "Files changed (0):");
    }
}
