//! Line/hunk-granular staging core (lane C1).
//!
//! Builds a minimal unified-diff patch for a selected subset of one file's
//! hunks (or of the +/- lines inside those hunks) and applies it to the index
//! with `Repository::apply(.., ApplyLocation::Index)` — the libgit2 equivalent
//! of `git apply --cached`. The worktree is never touched.
//!
//! Directions:
//! * stage   — source diff is index→workdir (preimage = index, postimage =
//!   workdir), so applying moves the selected changes INTO the index.
//! * unstage — source diff is HEAD-tree→index (preimage = HEAD, postimage =
//!   index). The output patch lines are reversed (origins and old/new numbers
//!   swapped) so the preimage becomes the index content and applying moves
//!   the index back toward HEAD for just that subset.
//!
//! Line-selection semantics (the `git add -p` model our diff viewer follows):
//! * context lines are always kept;
//! * a `+` line is kept iff its new-side line number falls inside one of the
//!   requested ranges;
//! * a `-` line is kept iff the insertion run it is paired with (the `+` run
//!   immediately following its deletion run) has at least one kept line —
//!   this stages replacement edits as a unit;
//! * a pure deletion run (no following `+` run) is kept iff the new-side
//!   number of the line following it (or, at end of hunk, the line preceding
//!   it) falls inside a range.
//!
//! Hunk `@@` headers are recomputed from the kept lines, and
//! `\ No newline at end of file` markers are regenerated for whichever side's
//! final kept line lacks a trailing newline. Binary files are rejected.

use std::collections::BTreeMap;

use git2::{ApplyLocation, Diff, DiffOptions, Patch, Repository};

use crate::engine::git_engine::{EngineError, EngineResult};
use crate::engine::types::LineRange;

/// One selection within a file's current diff.
#[derive(Debug, Clone)]
pub(crate) enum Selection {
    /// Every line of the hunk with this index (0-based, in diff order).
    Hunk(u32),
    /// Only the selected new-side lines of the hunk with this index.
    Lines(u32, Vec<LineRange>),
}

/// One raw diff line of a hunk. `content` includes the trailing `\n` when
/// the source file had one; `\ No newline` marker lines are dropped here and
/// regenerated when the output patch is rendered.
#[derive(Debug, Clone)]
struct RawLine {
    origin: char,
    old_no: Option<u32>,
    new_no: Option<u32>,
    content: Vec<u8>,
    kept: bool,
}

/// A hunk's raw lines plus the source hunk header anchors.
struct RawHunk {
    old_start: u32,
    new_start: u32,
    lines: Vec<RawLine>,
}

/// One hunk rendered for the output patch.
struct RenderedHunk {
    old_count: u32,
    new_count: u32,
    body: Vec<u8>,
}

/// The subset of one hunk to apply, keyed by hunk index. Selections for the
/// same hunk merge; a `Hunk` selection makes the union cover everything.
#[derive(Default)]
struct HunkSubset {
    whole: bool,
    ranges: Vec<LineRange>,
}

fn ranges_contain(ranges: &[LineRange], no: Option<u32>) -> bool {
    let Some(no) = no else { return false };
    ranges.iter().any(|r| no >= r.start && no <= r.end)
}

fn validate_ranges(ranges: &[LineRange]) -> EngineResult<()> {
    for r in ranges {
        if r.start == 0 || r.end < r.start {
            return Err(EngineError::Invalid(format!(
                "invalid line range {}..{} (1-based, inclusive)",
                r.start, r.end
            )));
        }
    }
    Ok(())
}

/// Stage (or unstage) the given selections for `path` against the index.
///
/// All selections must refer to hunks of the file's CURRENT diff in the
/// chosen direction; they are merged into one patch and applied in a single
/// `git apply --cached` so hunk indices stay stable relative to that diff.
pub(crate) fn apply_selection(
    repo: &Repository,
    path: &str,
    selections: &[Selection],
    unstage: bool,
) -> EngineResult<()> {
    // Merge selections per hunk index.
    let mut subsets: BTreeMap<u32, HunkSubset> = BTreeMap::new();
    for selection in selections {
        match selection {
            Selection::Hunk(h) => {
                subsets.entry(*h).or_default().whole = true;
            }
            Selection::Lines(h, ranges) => {
                validate_ranges(ranges)?;
                subsets
                    .entry(*h)
                    .or_default()
                    .ranges
                    .extend(ranges.iter().cloned());
            }
        }
    }
    if subsets.is_empty() {
        return Ok(());
    }

    let (diff, delta_idx) = source_diff(repo, path, unstage)?;
    // Generate the patch first: like the M1 read path, this is what
    // populates the delta's BINARY flag (checked right after).
    let patch = Patch::from_diff(&diff, delta_idx)?;
    let delta = diff.get_delta(delta_idx).expect("chosen delta exists");
    if delta.flags().contains(git2::DiffFlags::BINARY) {
        return Err(EngineError::Invalid(format!(
            "binary hunk staging unsupported: {path}"
        )));
    }

    let mut hunks: Vec<RawHunk> = match &patch {
        Some(patch) => raw_hunks(patch)?,
        None => Vec::new(),
    };
    // Untracked deltas produce an empty (or missing) patch: synthesize the
    // all-added hunk from the workdir file.
    if hunks.is_empty() && delta.status() == git2::Delta::Untracked {
        hunks = untracked_hunks(repo, path)?;
    }

    // Mark kept lines per selected hunk, then render each into patch bytes.
    let mut rendered: Vec<RenderedHunk> = Vec::new();
    for (hunk_idx, hunk) in hunks.iter_mut().enumerate() {
        let Some(subset) = subsets.get(&(hunk_idx as u32)) else {
            continue;
        };
        mark_kept(hunk, subset);
        if let Some(r) = render_hunk(hunk, unstage) {
            rendered.push(r);
        }
    }
    if rendered.is_empty() {
        return Ok(()); // selection matched no +/- line: nothing to stage
    }

    // File-level sides: empty preimage = new-file patch, empty postimage =
    // deleted-file patch.
    let old_total: u32 = rendered.iter().map(|r| r.old_count).sum();
    let new_total: u32 = rendered.iter().map(|r| r.new_count).sum();
    let (old_mode, new_mode) = if unstage {
        // Output old side = source new side (the index), and vice versa.
        (
            side_mode(delta.new_file().mode()),
            side_mode(delta.old_file().mode()),
        )
    } else {
        (
            side_mode(delta.old_file().mode()),
            side_mode(delta.new_file().mode()),
        )
    };

    let mut out = Vec::new();
    out.extend_from_slice(format!("diff --git a/{path} b/{path}\n").as_bytes());
    if old_total == 0 {
        out.extend_from_slice(format!("new file mode {new_mode:o}\n").as_bytes());
    } else if new_total == 0 {
        out.extend_from_slice(format!("deleted file mode {old_mode:o}\n").as_bytes());
    } else if old_mode != new_mode && new_mode != 0 {
        out.extend_from_slice(format!("old mode {old_mode:o}\n").as_bytes());
        out.extend_from_slice(format!("new mode {new_mode:o}\n").as_bytes());
    }
    let old_label = if old_total == 0 {
        "/dev/null".to_string()
    } else {
        format!("a/{path}")
    };
    let new_label = if new_total == 0 {
        "/dev/null".to_string()
    } else {
        format!("b/{path}")
    };
    out.extend_from_slice(format!("--- {old_label}\n").as_bytes());
    out.extend_from_slice(format!("+++ {new_label}\n").as_bytes());
    for hunk in &rendered {
        out.extend_from_slice(&hunk.body);
    }

    let diff = Diff::from_buffer(&out)
        .map_err(|e| EngineError::Invalid(format!("built patch for `{path}` is invalid: {e}")))?;
    repo.apply(&diff, ApplyLocation::Index, None)?;
    Ok(())
}

/// The pathspec-filtered source diff for `path` plus the index of its first
/// matching delta. The caller extracts the delta/patch in its own scope (a
/// `Patch` for an untracked workdir file comes back `None`; the hunks are
/// then synthesized from the file instead).
fn source_diff<'r>(
    repo: &'r Repository,
    path: &str,
    unstage: bool,
) -> EngineResult<(Diff<'r>, usize)> {
    let mut opts = DiffOptions::new();
    opts.pathspec(path).include_untracked(true);
    let diff = if unstage {
        let head_tree = match repo.head() {
            Ok(head) => Some(head.peel_to_tree()?),
            // Unborn HEAD reports EUNBORNBRANCH; diff against the empty tree.
            Err(e)
                if matches!(
                    e.code(),
                    git2::ErrorCode::NotFound | git2::ErrorCode::UnbornBranch
                ) =>
            {
                None
            }
            Err(e) => return Err(e.into()),
        };
        repo.diff_tree_to_index(head_tree.as_ref(), None, Some(&mut opts))?
    } else {
        repo.diff_index_to_workdir(None, Some(&mut opts))?
    };

    for idx in 0..diff.deltas().len() {
        let delta = diff.get_delta(idx).expect("delta count checked");
        let matches = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy() == path)
            .unwrap_or(false);
        if matches {
            return Ok((diff, idx));
        }
    }
    Err(EngineError::Invalid(format!(
        "no {} changes at `{path}`",
        if unstage { "staged" } else { "unstaged" }
    )))
}

fn raw_hunks(patch: &Patch<'_>) -> EngineResult<Vec<RawHunk>> {
    let mut out = Vec::new();
    for h in 0..patch.num_hunks() {
        let (header, line_count) = patch.hunk(h)?;
        let mut lines = Vec::with_capacity(line_count);
        for l in 0..line_count {
            let line = patch.line_in_hunk(h, l)?;
            let origin = line.origin();
            if !matches!(origin, ' ' | '+' | '-') {
                continue; // "\ No newline at end of file" markers
            }
            lines.push(RawLine {
                origin,
                old_no: line.old_lineno(),
                new_no: line.new_lineno(),
                content: line.content().to_vec(),
                kept: false,
            });
        }
        out.push(RawHunk {
            old_start: header.old_start(),
            new_start: header.new_start(),
            lines,
        });
    }
    Ok(out)
}

/// Untracked files carry no libgit2 patch: synthesize a single all-added
/// hunk from the workdir content.
fn untracked_hunks(repo: &Repository, path: &str) -> EngineResult<Vec<RawHunk>> {
    let workdir = repo
        .workdir()
        .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?;
    let bytes = std::fs::read(workdir.join(path))?;
    if bytes.contains(&0) {
        return Err(EngineError::Invalid(format!(
            "binary hunk staging unsupported: {path}"
        )));
    }
    let mut lines = Vec::new();
    let mut rest = bytes.as_slice();
    let mut new_no = 1u32;
    while !rest.is_empty() {
        let (line, tail) = match rest.iter().position(|&b| b == b'\n') {
            Some(pos) => (&rest[..=pos], &rest[pos + 1..]),
            None => (rest, &[][..]),
        };
        lines.push(RawLine {
            origin: '+',
            old_no: None,
            new_no: Some(new_no),
            content: line.to_vec(),
            kept: false,
        });
        new_no += 1;
        rest = tail;
    }
    Ok(vec![RawHunk {
        old_start: 0,
        new_start: 1,
        lines,
    }])
}

/// Mark `kept` on the hunk's lines according to the selection semantics
/// documented at the top of this module.
fn mark_kept(hunk: &mut RawHunk, subset: &HunkSubset) {
    if subset.whole {
        for line in &mut hunk.lines {
            line.kept = true;
        }
        return;
    }
    let ranges = &subset.ranges;
    // Pass 1: context always kept, '+' lines by new-side number.
    for line in &mut hunk.lines {
        line.kept = match line.origin {
            ' ' => true,
            '+' => ranges_contain(ranges, line.new_no),
            _ => false,
        };
    }
    // Pass 2: '-' runs. A deletion run paired with a following '+' run is
    // kept iff any of those '+' lines is kept; a pure deletion run is kept
    // iff the nearest new-side number around it is selected.
    let n = hunk.lines.len();
    let mut i = 0;
    while i < n {
        if hunk.lines[i].origin != '-' {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < n && hunk.lines[i].origin == '-' {
            i += 1;
        }
        let del_end = i; // exclusive
        let paired_plus = i < n && hunk.lines[i].origin == '+';
        let keep_run = if paired_plus {
            let mut any = false;
            while i < n && hunk.lines[i].origin == '+' {
                any |= hunk.lines[i].kept;
                i += 1;
            }
            any
        } else {
            // Pure deletion run: nearest new-side number, forward first.
            let near = hunk.lines[del_end..]
                .iter()
                .find_map(|l| l.new_no)
                .or_else(|| hunk.lines[..del_start].iter().rev().find_map(|l| l.new_no));
            ranges_contain(ranges, near)
        };
        for line in &mut hunk.lines[del_start..del_end] {
            line.kept = keep_run;
        }
    }
}

/// Render the kept subset of one hunk as output-patch bytes (recomputed `@@`
/// header + body), reversing line directions for the unstage flow. `None`
/// when no +/- line is kept (nothing to apply for this hunk).
fn render_hunk(hunk: &RawHunk, unstage: bool) -> Option<RenderedHunk> {
    struct OutLine {
        origin: char,
        old_no: Option<u32>,
        new_no: Option<u32>,
        content: Vec<u8>,
    }
    let out_lines: Vec<OutLine> = hunk
        .lines
        .iter()
        .filter(|l| l.kept)
        .map(|l| {
            if unstage {
                OutLine {
                    origin: match l.origin {
                        '+' => '-',
                        '-' => '+',
                        other => other,
                    },
                    old_no: l.new_no,
                    new_no: l.old_no,
                    content: l.content.clone(),
                }
            } else {
                OutLine {
                    origin: l.origin,
                    old_no: l.old_no,
                    new_no: l.new_no,
                    content: l.content.clone(),
                }
            }
        })
        .collect();
    if !out_lines.iter().any(|l| matches!(l.origin, '+' | '-')) {
        return None;
    }

    let old_count = out_lines
        .iter()
        .filter(|l| matches!(l.origin, ' ' | '-'))
        .count() as u32;
    let new_count = out_lines
        .iter()
        .filter(|l| matches!(l.origin, ' ' | '+'))
        .count() as u32;

    // Header anchors. A non-empty output side starts at its first line's
    // number; an empty side uses git's `start,0` convention — the position
    // AFTER which the change hangs (0 = top of file). In the unstage flow
    // the sides swap: output old tracks the source's new side.
    let old_start = if old_count > 0 {
        out_lines
            .iter()
            .find(|l| matches!(l.origin, ' ' | '-'))
            .and_then(|l| l.old_no)
            .unwrap_or(1)
    } else {
        empty_side_start(hunk, unstage, false)
    };
    let new_start = if new_count > 0 {
        out_lines
            .iter()
            .find(|l| matches!(l.origin, ' ' | '+'))
            .and_then(|l| l.new_no)
            .unwrap_or(1)
    } else {
        empty_side_start(hunk, unstage, true)
    };

    let mut body = Vec::new();
    body.extend_from_slice(
        format!("@@ -{old_start},{old_count} +{new_start},{new_count} @@\n").as_bytes(),
    );
    for line in &out_lines {
        body.push(line.origin as u8);
        body.extend_from_slice(&line.content);
        // A line without a trailing newline is by definition the last line
        // of its side; git marks it immediately after the line itself.
        if !line.content.ends_with(b"\n") {
            body.extend_from_slice(b"\\ No newline at end of file\n");
        }
    }
    Some(RenderedHunk {
        old_count,
        new_count,
        body,
    })
}

/// Start position for an output side that kept no lines of its own (`n,0`
/// in the `@@` header): the last tracked source line before the first kept
/// line; if the change hangs before the hunk's first tracked line, that
/// line's number minus one; if the source hunk tracks no line at all (pure
/// insertion hunks), git's own header anchor already IS the after-position.
///
/// `side_new` picks the output side; the tracked source side is the same
/// side when staging and the opposite side when unstaging.
fn empty_side_start(hunk: &RawHunk, unstage: bool, side_new: bool) -> u32 {
    let tracked_old = side_new == unstage; // output side -> tracked source side
    let tracked_no = |l: &RawLine| if tracked_old { l.old_no } else { l.new_no };
    let is_tracked = |l: &RawLine| match l.origin {
        ' ' => true,
        '+' => !tracked_old,
        '-' => tracked_old,
        _ => false,
    };
    let anchor = if tracked_old {
        hunk.old_start
    } else {
        hunk.new_start
    };

    let mut before_first_kept: Option<u32> = None;
    let mut first_tracked: Option<u32> = None;
    let mut seen_kept = false;
    for line in &hunk.lines {
        if line.kept {
            seen_kept = true;
        }
        if is_tracked(line) {
            let no = tracked_no(line).unwrap_or(anchor);
            if first_tracked.is_none() {
                first_tracked = Some(no);
            }
            if !seen_kept {
                before_first_kept = Some(no);
            }
        }
    }
    if let Some(prev) = before_first_kept {
        return prev;
    }
    match first_tracked {
        // Change hangs immediately before the hunk's first tracked line.
        Some(first) => first.saturating_sub(1),
        // Pure insertion hunk: the anchor is git's after-position.
        None => anchor,
    }
}

/// File mode as an octal-printable u32 (`0o100644` etc.).
fn side_mode(mode: git2::FileMode) -> u32 {
    i32::from(mode) as u32
}
