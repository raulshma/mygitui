//! `Libgit2Engine`: the primary `GitEngine` implementation, backed by git2-rs.
//!
//! All operations are synchronous (git2 is not async); IPC commands wrap them
//! in `tauri::async_runtime::spawn_blocking`. `CliEngine` (system git) exists
//! as a fallback for capabilities libgit2 cannot reach — everything in this
//! module is what the M1 read path needs.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use git2::{
    BlameOptions, Branch, Commit, Delta, Diff, DiffFindOptions, DiffOptions, Oid, Patch,
    Repository, RepositoryState, Signature, Sort, StatusOptions,
};
use imara_diff::{Algorithm, Diff as WordDiff, InternedInput, TokenSource};

use super::git_engine::{EngineError, EngineResult, FetchProgress, GitEngine, PushProgress};
use super::types::{
    BlameLine, BranchInfo, ChangeKind, CommitInfo, CommitOptions, DiffHunk, DiffLine, DiffSide,
    FetchOptions, FileDiff, GitSignature, HookInfo, LogFilter, NetStats, PullOptions, PushOptions,
    RemoteBranchInfo, RemoteInfo, RepoStatus, SigningInfo, StageRequest, StageTarget, StatusEntry,
    TagInfo,
};

/// Lines longer than this (bytes) skip the imara-diff word pass; the whole
/// line is already visually marked as changed.
const WORD_DIFF_MAX_BYTES: usize = 1000;

/// Safety cap on how many commits `log` will scan past while looking for the
/// `after` pagination cursor before giving up.
const LOG_CURSOR_SCAN_LIMIT: usize = 100_000;

/// File extensions treated as images for diff rendering.
const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif",
];

pub struct Libgit2Engine;

impl Libgit2Engine {
    pub fn new() -> Self {
        Libgit2Engine
    }
}

impl Default for Libgit2Engine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// status helpers
// ---------------------------------------------------------------------------

fn kind_from_status(delta_status: git2::Delta) -> ChangeKind {
    match delta_status {
        Delta::Added => ChangeKind::Added,
        Delta::Deleted => ChangeKind::Deleted,
        Delta::Modified => ChangeKind::Modified,
        Delta::Renamed => ChangeKind::Renamed,
        Delta::Copied => ChangeKind::Copied,
        Delta::Untracked => ChangeKind::Untracked,
        Delta::Conflicted => ChangeKind::Conflicted,
        Delta::Typechange => ChangeKind::Modified,
        _ => ChangeKind::Modified,
    }
}

fn delta_path(delta: &git2::DiffDelta<'_>) -> Option<String> {
    delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .map(|p| p.to_string_lossy().into_owned())
}

fn delta_old_path(delta: &git2::DiffDelta<'_>) -> Option<String> {
    match delta.status() {
        Delta::Renamed | Delta::Copied | Delta::Deleted => delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned()),
        _ => None,
    }
}

/// Branch name of HEAD, also handling unborn HEAD (symbolic ref only).
fn head_branch_name(repo: &Repository) -> Option<String> {
    if let Ok(head) = repo.head() {
        return head.shorthand().ok().map(str::to_owned);
    }
    let sym = repo
        .find_reference("HEAD")
        .ok()
        .and_then(|r| r.symbolic_target().ok().flatten().map(str::to_owned))?;
    sym.rsplit('/').next().map(str::to_owned)
}

/// ahead/behind of HEAD's branch vs its configured upstream (0/0 when none).
fn ahead_behind(repo: &Repository) -> (u32, u32) {
    let head = match repo.head() {
        Ok(h) => h,
        Err(_) => return (0, 0),
    };
    let local = match head.target() {
        Some(oid) => oid,
        None => return (0, 0),
    };
    let upstream = match Branch::wrap(head).upstream() {
        Ok(up) => up,
        Err(_) => return (0, 0),
    };
    let remote_oid = match upstream.get().target() {
        Some(oid) => oid,
        None => return (0, 0),
    };
    match repo.graph_ahead_behind(local, remote_oid) {
        Ok((ahead, behind)) => (ahead as u32, behind as u32),
        Err(_) => (0, 0),
    }
}

fn in_progress_state(repo: &Repository) -> (bool, bool, bool) {
    let gitdir = repo.path();
    let state = repo.state();

    let rebasing = gitdir.join("rebase-merge").is_dir()
        || gitdir.join("rebase-apply").is_dir()
        // mygitui's custom rebase sequencer (engine/rebase.rs) persists its
        // state as JSON instead of git's rebase dirs.
        || gitdir.join("mygitui").join("rebase.json").exists()
        || matches!(
            state,
            RepositoryState::Rebase
                | RepositoryState::RebaseInteractive
                | RepositoryState::RebaseMerge
                | RepositoryState::ApplyMailbox
                | RepositoryState::ApplyMailboxOrRebase
        );
    let merging = gitdir.join("MERGE_HEAD").exists() || state == RepositoryState::Merge;
    // During a rebase, cherry-pick/revert marker files may also be present;
    // report only the rebase in that case (matches `git status`).
    let sequencer = !rebasing
        && (gitdir.join("sequencer").is_dir()
            || gitdir.join("CHERRY_PICK_HEAD").exists()
            || gitdir.join("REVERT_HEAD").exists()
            || matches!(
                state,
                RepositoryState::CherryPick
                    | RepositoryState::CherryPickSequence
                    | RepositoryState::Revert
                    | RepositoryState::RevertSequence
            ));
    (merging, rebasing, sequencer)
}

// ---------------------------------------------------------------------------
// shared helpers
// ---------------------------------------------------------------------------

fn sig_to_model(sig: Signature<'_>) -> GitSignature {
    GitSignature {
        name: sig.name().unwrap_or_default().to_owned(),
        email: sig.email().unwrap_or_default().to_owned(),
        time: sig.when().seconds(),
        offset_minutes: sig.when().offset_minutes(),
    }
}

fn sig_opt_to_model(sig: Option<Signature<'_>>) -> GitSignature {
    sig.map(sig_to_model).unwrap_or(GitSignature {
        name: String::new(),
        email: String::new(),
        time: 0,
        offset_minutes: 0,
    })
}

/// `refs/heads/main` -> `main`, `refs/remotes/origin/main` -> `origin/main`,
/// `refs/tags/v1.0.0` -> `v1.0.0`; `None` for anything else.
fn short_ref_name(full: &str) -> Option<&str> {
    ["refs/heads/", "refs/remotes/", "refs/tags/"]
        .iter()
        .find_map(|prefix| full.strip_prefix(prefix))
}

fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Component-wise prefix match: `src` matches `src` and `src/main.py`
/// but not `srcx/file.py`.
fn path_matches(path: &str, filter: &str) -> bool {
    let filter = filter.trim_end_matches(['/', '\\']);
    if filter.is_empty() {
        return true;
    }
    path == filter || path.starts_with(&format!("{filter}/"))
}

fn paths_allow(path: Option<&str>, old_path: Option<&str>, paths: Option<&[String]>) -> bool {
    let Some(filters) = paths else {
        return true;
    };
    if filters.is_empty() {
        return true;
    }
    filters.iter().any(|f| {
        path.map(|p| path_matches(p, f)).unwrap_or(false)
            || old_path.map(|p| path_matches(p, f)).unwrap_or(false)
    })
}

/// Resolve a commit-ish string to a commit (propagates errors as `Invalid`).
fn resolve_commit<'r>(repo: &'r Repository, spec: &str) -> EngineResult<Commit<'r>> {
    let obj = repo
        .revparse_single(spec)
        .map_err(|e| EngineError::Invalid(format!("cannot resolve `{spec}`: {e}")))?;
    obj.peel_to_commit()
        .map_err(|e| EngineError::Invalid(format!("`{spec}` is not a commit: {e}")))
}

/// Resolve a diff side to a tree. `None` means "empty tree" (unborn HEAD).
fn side_tree<'r>(repo: &'r Repository, side: &DiffSide) -> EngineResult<Option<git2::Tree<'r>>> {
    match side {
        DiffSide::Worktree | DiffSide::Index => Ok(None),
        DiffSide::Head => match repo.head() {
            Ok(head) => Ok(Some(head.peel_to_tree()?)),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        },
        DiffSide::Commit(spec) => {
            let commit = resolve_commit(repo, spec)?;
            let tree = commit
                .tree()
                .map_err(|e| EngineError::Invalid(format!("`{spec}` has no tree: {e}")))?;
            Ok(Some(tree))
        }
    }
}

fn tree_of<'r>(commit: &Commit<'r>) -> EngineResult<git2::Tree<'r>> {
    Ok(commit.tree()?)
}

/// sha -> ref-name decorations ("main", "origin/main", "v1.0.0",
/// "HEAD -> main"), used by `log`.
fn decorations(repo: &Repository) -> EngineResult<HashMap<Oid, Vec<String>>> {
    let mut map: HashMap<Oid, Vec<String>> = HashMap::new();
    for reference in repo.references()?.flatten() {
        let Ok(name) = reference.name() else {
            continue;
        };
        let Some(short) = short_ref_name(name) else {
            continue;
        };
        if let Ok(commit) = reference.peel_to_commit() {
            map.entry(commit.id()).or_default().push(short.to_owned());
        }
    }
    // HEAD decoration.
    if let Ok(head) = repo.head() {
        if let Some(target) = head.target() {
            let label = if repo.head_detached().unwrap_or(false) {
                "HEAD".to_string()
            } else {
                format!("HEAD -> {}", head.shorthand().unwrap_or("HEAD"))
            };
            map.entry(target).or_default().push(label);
        }
    }
    // Deterministic order: HEAD decoration first, then alphabetical.
    for names in map.values_mut() {
        names.sort_by(|a, b| {
            let ah = a.starts_with("HEAD");
            let bh = b.starts_with("HEAD");
            bh.cmp(&ah).then_with(|| a.cmp(b))
        });
    }
    Ok(map)
}

fn commit_info(commit: &Commit<'_>, decos: &HashMap<Oid, Vec<String>>) -> CommitInfo {
    let message = commit.message().unwrap_or_default().to_owned();
    let summary = match commit.summary() {
        Ok(Some(s)) => s.to_owned(),
        _ => message.lines().next().unwrap_or_default().to_owned(),
    };
    CommitInfo {
        sha: commit.id().to_string(),
        parents: commit.parent_ids().map(|p| p.to_string()).collect(),
        author: sig_to_model(commit.author()),
        committer: sig_to_model(commit.committer()),
        message,
        summary,
        refs: decos.get(&commit.id()).cloned().unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// word-level highlights (imara-diff)
// ---------------------------------------------------------------------------

/// Split a line into word-ish tokens with their byte ranges. A token is a
/// maximal run of alphanumeric/underscore characters, or a maximal run of
/// everything else (punctuation, whitespace).
fn split_word_tokens(line: &str) -> (Vec<&str>, Vec<(u32, u32)>) {
    let mut words = Vec::new();
    let mut ranges = Vec::new();
    let mut start = 0usize;
    let mut current_is_word: Option<bool> = None;
    for (idx, ch) in line.char_indices() {
        let is_word = ch.is_alphanumeric() || ch == '_';
        match current_is_word {
            None => {
                start = idx;
                current_is_word = Some(is_word);
            }
            Some(prev) if prev == is_word => {}
            Some(_) => {
                words.push(&line[start..idx]);
                ranges.push((start as u32, idx as u32));
                start = idx;
                current_is_word = Some(is_word);
            }
        }
    }
    if current_is_word.is_some() {
        words.push(&line[start..]);
        ranges.push((start as u32, line.len() as u32));
    }
    (words, ranges)
}

/// A precomputed list of word tokens exposed to imara-diff.
struct WordList<'a>(&'a [&'a str]);

impl<'a> TokenSource for WordList<'a> {
    type Token = &'a str;
    type Tokenizer = std::iter::Copied<std::slice::Iter<'a, &'a str>>;

    fn tokenize(&self) -> Self::Tokenizer {
        self.0.iter().copied()
    }

    fn estimate_tokens(&self) -> u32 {
        self.0.len() as u32
    }
}

/// Byte ranges (start, end) for word-level highlighting.
type ByteRanges = Vec<(u32, u32)>;

/// Byte ranges of the changed words on each side of a modified line pair.
fn word_highlights(old: &str, new: &str) -> (ByteRanges, ByteRanges) {
    if old.len() > WORD_DIFF_MAX_BYTES || new.len() > WORD_DIFF_MAX_BYTES {
        return (Vec::new(), Vec::new());
    }
    let (old_words, old_ranges) = split_word_tokens(old);
    let (new_words, new_ranges) = split_word_tokens(new);
    if old_words.is_empty() && new_words.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if old_words.is_empty() || new_words.is_empty() {
        // One side has no words at all: highlight the whole other line.
        return (full_line_range(old), full_line_range(new));
    }

    let input = InternedInput::new(WordList(&old_words), WordList(&new_words));
    let mut diff = WordDiff::compute(Algorithm::Myers, &input);
    diff.postprocess_no_heuristic(&input);

    let mut old_out = Vec::new();
    let mut new_out = Vec::new();
    for hunk in diff.hunks() {
        for t in hunk.before.start..hunk.before.end {
            let i = t as usize;
            if !old_words[i].trim().is_empty() {
                old_out.push(old_ranges[i]);
            }
        }
        for t in hunk.after.start..hunk.after.end {
            let i = t as usize;
            if !new_words[i].trim().is_empty() {
                new_out.push(new_ranges[i]);
            }
        }
    }
    (merge_ranges(old_out), merge_ranges(new_out))
}

/// Highlight range covering the non-whitespace part of a line.
fn full_line_range(text: &str) -> ByteRanges {
    let bytes = text.as_bytes();
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|p| p + 1)
        .unwrap_or(0);
    if start < end {
        vec![(start as u32, end as u32)]
    } else {
        Vec::new()
    }
}

/// Merge touching ranges so renderers draw one block per changed region.
fn merge_ranges(mut ranges: ByteRanges) -> ByteRanges {
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some((_, last_end)) if *last_end >= start => {
                *last_end = (*last_end).max(end);
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Fill word-level highlights for the '-'/'+' runs of one hunk's lines.
fn highlight_hunk(lines: &mut [DiffLine]) {
    let mut i = 0;
    while i < lines.len() {
        if lines[i].origin != '-' {
            i += 1;
            continue;
        }
        let del_start = i;
        while i < lines.len() && lines[i].origin == '-' {
            i += 1;
        }
        let del_end = i;
        while i < lines.len() && lines[i].origin == '+' {
            i += 1;
        }
        let add_end = i;

        let dels = &lines[del_start..del_end];
        let adds = &lines[del_end..add_end];
        let pairs = dels.len().max(adds.len());
        // Compute immutable first, then assign: two live `&mut` sub-slices of
        // `lines` would not pass the borrow checker.
        let highlights: Vec<(ByteRanges, ByteRanges)> = (0..pairs)
            .map(|k| match (dels.get(k), adds.get(k)) {
                (Some(old), Some(new)) => word_highlights(&old.text, &new.text),
                (Some(line), None) => (full_line_range(&line.text), Vec::new()),
                (None, Some(line)) => (Vec::new(), full_line_range(&line.text)),
                (None, None) => (Vec::new(), Vec::new()),
            })
            .collect();
        for (k, (old_highlights, new_highlights)) in highlights.into_iter().enumerate() {
            if let Some(line) = lines.get_mut(del_start + k) {
                line.highlights = old_highlights;
            }
            if let Some(line) = lines.get_mut(del_end + k) {
                line.highlights = new_highlights;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// diff conversion
// ---------------------------------------------------------------------------

fn build_diff<'r>(repo: &'r Repository, old: &DiffSide, new: &DiffSide) -> EngineResult<Diff<'r>> {
    let mut opts = DiffOptions::new();
    if matches!(new, DiffSide::Worktree) {
        // GUI parity: show untracked files as additions in worktree diffs.
        opts.include_untracked(true).recurse_untracked_dirs(true);
    }
    let mut diff = match (old, new) {
        (DiffSide::Index, DiffSide::Worktree) => {
            repo.diff_index_to_workdir(None, Some(&mut opts))?
        }
        (DiffSide::Index, DiffSide::Commit(_) | DiffSide::Head) => {
            // "What changed since <commit>?" over staged content: materialize
            // the index to a tree and diff tree-to-tree (git writes the same
            // throwaway tree object for `git diff --cached <commit>`).
            let index_tree_oid = repo.index()?.write_tree()?;
            let index_tree = repo.find_tree(index_tree_oid)?;
            let new_tree = side_tree(repo, new)?;
            repo.diff_tree_to_tree(Some(&index_tree), new_tree.as_ref(), Some(&mut opts))?
        }
        (_, DiffSide::Index) => {
            let old_tree = side_tree(repo, old)?;
            repo.diff_tree_to_index(old_tree.as_ref(), None, Some(&mut opts))?
        }
        (_, DiffSide::Worktree) => {
            let old_tree = side_tree(repo, old)?;
            repo.diff_tree_to_workdir_with_index(old_tree.as_ref(), Some(&mut opts))?
        }
        (_, DiffSide::Commit(_) | DiffSide::Head) => {
            let old_tree = side_tree(repo, old)?;
            let new_tree = side_tree(repo, new)?;
            repo.diff_tree_to_tree(old_tree.as_ref(), new_tree.as_ref(), Some(&mut opts))?
        }
    };
    // Rename detection (`git diff --find-renames` behavior); libgit2 does
    // not run it implicitly.
    let mut find = DiffFindOptions::new();
    find.renames(true).copies(false);
    diff.find_similar(Some(&mut find))?;
    Ok(diff)
}

/// Untracked files carry no patch from libgit2; build an "all added" hunk by
/// reading the workdir file. Returns `Err(())`-style `None` with `binary`
/// reported for files with NUL bytes, and skips files larger than 1 MiB.
const UNTRACKED_SYNTH_LIMIT: u64 = 1024 * 1024;

fn untracked_file_hunk(repo: &Repository, path: &str) -> Option<(bool, Vec<DiffHunk>, u32)> {
    let workdir = repo.workdir()?;
    let file_path = workdir.join(path);
    let Ok(meta) = std::fs::metadata(&file_path) else {
        return None;
    };
    if meta.len() > UNTRACKED_SYNTH_LIMIT {
        return None;
    }
    let Ok(bytes) = std::fs::read(&file_path) else {
        return None;
    };
    if bytes.contains(&0) {
        return Some((true, Vec::new(), 0)); // binary
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut split: Vec<&str> = text.split('\n').collect();
    if text.ends_with('\n') {
        split.pop();
    }
    let mut lines = Vec::with_capacity(split.len());
    for (i, line) in split.iter().enumerate() {
        lines.push(DiffLine {
            old_no: None,
            new_no: Some(i as u32 + 1),
            origin: '+',
            text: line.trim_end_matches('\r').to_owned(),
            highlights: Vec::new(),
        });
    }
    let additions = lines.len() as u32;
    let hunks = vec![DiffHunk {
        old_start: 0,
        new_start: 1,
        lines,
    }];
    Some((false, hunks, additions))
}

fn diff_to_files(
    repo: &Repository,
    diff: &Diff<'_>,
    paths: Option<&[String]>,
) -> EngineResult<Vec<FileDiff>> {
    let mut files = Vec::new();
    for idx in 0..diff.deltas().len() {
        // Build the patch first: this also populates binary detection flags.
        let patch = Patch::from_diff(diff, idx)?;
        let Some(delta) = diff.get_delta(idx) else {
            continue;
        };
        let path = delta_path(&delta);
        let old_path = delta_old_path(&delta);
        if !paths_allow(path.as_deref(), old_path.as_deref(), paths) {
            continue;
        }
        let path = path.unwrap_or_default();

        let mut binary = delta.flags().contains(git2::DiffFlags::BINARY);
        let is_image = is_image_path(&path);
        let mut hunks = Vec::new();
        let mut additions = 0u32;
        let mut deletions = 0u32;

        if !binary {
            if let Some(patch) = patch {
                for h in 0..patch.num_hunks() {
                    let (hunk, line_count) = patch.hunk(h)?;
                    let mut lines = Vec::with_capacity(line_count);
                    for l in 0..line_count {
                        let line = patch.line_in_hunk(h, l)?;
                        let origin = line.origin();
                        if !matches!(origin, '+' | '-' | ' ') {
                            continue; // e.g. "\ No newline at end of file"
                        }
                        if origin == '+' {
                            additions += 1;
                        } else if origin == '-' {
                            deletions += 1;
                        }
                        let text = String::from_utf8_lossy(line.content());
                        let text = text.trim_end_matches(['\n', '\r']).to_owned();
                        lines.push(DiffLine {
                            old_no: line.old_lineno(),
                            new_no: line.new_lineno(),
                            origin,
                            text,
                            highlights: Vec::new(),
                        });
                    }
                    highlight_hunk(&mut lines);
                    hunks.push(DiffHunk {
                        old_start: hunk.old_start(),
                        new_start: hunk.new_start(),
                        lines,
                    });
                }
            }
        }

        // Untracked deltas get no patch from libgit2: synthesize an all-added
        // hunk from the workdir file so the UI has content to show.
        if delta.status() == Delta::Untracked && hunks.is_empty() && !binary {
            if let Some((is_binary, synth, adds)) = untracked_file_hunk(repo, &path) {
                binary = is_binary;
                additions = adds;
                hunks = synth;
            }
        }

        files.push(FileDiff {
            path,
            old_path,
            binary,
            is_image,
            additions,
            deletions,
            hunks,
        });
    }
    Ok(files)
}

// ---------------------------------------------------------------------------
// log path filtering
// ---------------------------------------------------------------------------

/// Does `commit` change anything at `path` (plain pathspec diff)?
fn commit_touches_path(repo: &Repository, commit: &Commit<'_>, path: &str) -> EngineResult<bool> {
    let mut opts = DiffOptions::new();
    opts.pathspec(path);
    let tree = tree_of(commit)?;
    let parent_tree = commit.parent(0).ok().map(|p| tree_of(&p)).transpose()?;
    let diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), Some(&mut opts))?;
    Ok(diff.deltas().len() > 0)
}

/// Pickaxe `-S` test: does the commit's patch add or remove `needle`?
/// Walks the patch lines of the (first-parent) tree diff; content is
/// scanned byte-wise (needle is UTF-8; the diff may not be).
fn commit_touches_string(
    repo: &Repository,
    commit: &Commit<'_>,
    needle: &str,
) -> EngineResult<bool> {
    let tree = tree_of(commit)?;
    let parent_tree = commit.parent(0).ok().map(|p| tree_of(&p)).transpose()?;
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    let mut find = DiffFindOptions::new();
    find.renames(true).copies(false);
    let _ = diff.find_similar(Some(&mut find));
    let needle_bytes = needle.as_bytes();
    for delta_idx in 0..diff.deltas().len() {
        let patch = match Patch::from_diff(&diff, delta_idx)? {
            Some(patch) => patch,
            None => continue, // binary delta
        };
        for hunk_idx in 0..patch.num_hunks() {
            for line_idx in 0..patch.num_lines_in_hunk(hunk_idx)? {
                let line = patch.line_in_hunk(hunk_idx, line_idx)?;
                let origin = line.origin();
                if origin != '+' && origin != '-' {
                    continue;
                }
                let content = line.content();
                if content.len() >= needle_bytes.len()
                    && content
                        .windows(needle_bytes.len())
                        .any(|window| window == needle_bytes)
                {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

/// Pickaxe `-G` test: does the commit's patch text (the added + removed
/// lines) match `re`? Same per-commit tree-diff walk as [`commit_touches_string`]
/// (M12); the +/- lines are joined into one buffer so the regex matches as
/// if the whole patch text were the haystack. Like git's `-G`, matching is
/// line-oriented: use `(?m)` anchors to bind to line boundaries (diff line
/// content already carries its trailing newline; a missing one is added so
/// lines stay separated).
fn commit_touches_regex(
    repo: &Repository,
    commit: &Commit<'_>,
    re: &regex::Regex,
) -> EngineResult<bool> {
    let tree = tree_of(commit)?;
    let parent_tree = commit.parent(0).ok().map(|p| tree_of(&p)).transpose()?;
    let mut diff = repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None)?;
    let mut find = DiffFindOptions::new();
    find.renames(true).copies(false);
    let _ = diff.find_similar(Some(&mut find));
    let mut patch_text = String::new();
    for delta_idx in 0..diff.deltas().len() {
        let patch = match Patch::from_diff(&diff, delta_idx)? {
            Some(patch) => patch,
            None => continue, // binary delta
        };
        for hunk_idx in 0..patch.num_hunks() {
            for line_idx in 0..patch.num_lines_in_hunk(hunk_idx)? {
                let line = patch.line_in_hunk(hunk_idx, line_idx)?;
                let origin = line.origin();
                if origin != '+' && origin != '-' {
                    continue;
                }
                let content = String::from_utf8_lossy(line.content()).into_owned();
                patch_text.push_str(&content);
                if !content.ends_with('\n') {
                    patch_text.push('\n');
                }
            }
        }
    }
    Ok(re.is_match(&patch_text))
}

/// `--follow` support: diff the commit against its first parent with rename
/// detection; if the followed path was renamed here, shift `current` to the
/// old name so older commits are matched under their historical name.
fn commit_follows_path(repo: &Repository, commit: &Commit<'_>, current: &mut String) -> bool {
    let tree = match tree_of(commit) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let parent_tree = commit.parent(0).ok().and_then(|p| tree_of(&p).ok());
    let mut diff = match repo.diff_tree_to_tree(parent_tree.as_ref(), Some(&tree), None) {
        Ok(d) => d,
        Err(_) => return false,
    };
    let mut find = DiffFindOptions::new();
    find.renames(true).copies(false);
    if diff.find_similar(Some(&mut find)).is_err() {
        return false;
    }
    let mut touched = false;
    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());
        if delta.status() == Delta::Renamed && new_path.as_deref() == Some(current.as_str()) {
            if let Some(old) = old_path {
                *current = old;
            }
            touched = true;
        } else if new_path.as_deref() == Some(current.as_str())
            || old_path.as_deref() == Some(current.as_str())
        {
            touched = true;
        }
    }
    touched
}

// ---------------------------------------------------------------------------
// GitEngine
// ---------------------------------------------------------------------------

impl GitEngine for Libgit2Engine {
    fn status(&self, repo: &Repository) -> EngineResult<RepoStatus> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .exclude_submodules(true);

        let statuses = repo.statuses(Some(&mut opts))?;
        let mut entries = Vec::with_capacity(statuses.len());
        for entry in statuses.iter() {
            let idx_delta = entry.head_to_index();
            let wt_delta = entry.index_to_workdir();

            let mut index = idx_delta
                .as_ref()
                .map(|d| kind_from_status(d.status()))
                .unwrap_or(ChangeKind::Unmodified);
            if entry.status().is_conflicted() {
                index = ChangeKind::Conflicted;
            }
            let worktree = wt_delta
                .as_ref()
                .map(|d| kind_from_status(d.status()))
                .unwrap_or(ChangeKind::Unmodified);

            if index == ChangeKind::Unmodified && worktree == ChangeKind::Unmodified {
                continue;
            }

            let path = idx_delta
                .as_ref()
                .and_then(|d| delta_path(d))
                .or_else(|| wt_delta.as_ref().and_then(|d| delta_path(d)))
                .unwrap_or_else(|| entry.path().map(str::to_owned).unwrap_or_default());
            let old_path = idx_delta
                .as_ref()
                .and_then(|d| delta_old_path(d))
                .or_else(|| wt_delta.as_ref().and_then(|d| delta_old_path(d)));

            entries.push(StatusEntry {
                path,
                old_path,
                index,
                worktree,
            });
        }

        let detached = repo.head_detached().unwrap_or(false);
        let branch = if detached {
            None
        } else {
            head_branch_name(repo)
        };
        let head = repo
            .head()
            .ok()
            .and_then(|h| h.target())
            .map(|oid| oid.to_string());
        let (ahead, behind) = ahead_behind(repo);
        let (merging, rebasing, sequencer) = in_progress_state(repo);

        Ok(RepoStatus {
            branch,
            head,
            detached,
            ahead,
            behind,
            merging,
            rebasing,
            sequencer,
            entries,
        })
    }

    fn diff(
        &self,
        repo: &Repository,
        old: &DiffSide,
        new: &DiffSide,
        paths: Option<&[String]>,
    ) -> EngineResult<Vec<FileDiff>> {
        match (old, new) {
            // The working directory cannot be the base side of a git diff,
            // and identical sides produce no deltas.
            (DiffSide::Worktree, _) | (DiffSide::Index, DiffSide::Index) => return Ok(Vec::new()),
            _ => {}
        }
        let diff = build_diff(repo, old, new)?;
        diff_to_files(repo, &diff, paths)
    }

    fn log(
        &self,
        repo: &Repository,
        filter: &LogFilter,
        limit: usize,
        after: Option<&str>,
    ) -> EngineResult<(Vec<CommitInfo>, Option<String>)> {
        if limit == 0 {
            return Ok((Vec::new(), after.map(str::to_owned)));
        }
        // Regex mode: compile once, case-sensitive (git `--grep` semantics);
        // invalid patterns fail fast with a friendly message.
        let text_regex = if filter.regex {
            filter
                .text
                .as_deref()
                .filter(|t| !t.is_empty())
                .map(|t| {
                    regex::Regex::new(t)
                        .map_err(|e| EngineError::Invalid(format!("invalid regex `{t}`: {e}")))
                })
                .transpose()?
        } else {
            None
        };
        // Pickaxe -G (M12): compiled once per walk, same fail-fast treatment
        // for invalid patterns as the `regex` log filter above.
        let pickaxe_regex = filter
            .pickaxe_regex
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(|r| {
                regex::Regex::new(r)
                    .map_err(|e| EngineError::Invalid(format!("invalid pickaxe regex `{r}`: {e}")))
            })
            .transpose()?;

        let decos = decorations(repo)?;
        let mut walk = repo.revwalk()?;
        // Date-prioritized topological order (git `--topo-order`): children
        // always precede parents, otherwise newest commits pop first. Plain
        // TOPOLOGICAL lets stale branch tips surface above HEAD.
        walk.set_sorting(Sort::TIME | Sort::TOPOLOGICAL)?;

        let mut pushed: HashSet<Oid> = HashSet::new();
        if filter.refs.is_empty() {
            if let Ok(head) = repo.head() {
                if let Some(oid) = head.target() {
                    walk.push(oid)?;
                    pushed.insert(oid);
                }
            }
            for reference in repo.references()?.flatten() {
                // Stash commits live in the Stash panel: interleaving the
                // merge-shaped WIP/index/untracked triples here derails the
                // commit-graph lanes (their base commit is often weeks old,
                // holding a lane open across the whole graph).
                if reference
                    .name()
                    .is_ok_and(|n| n.starts_with("refs/stash"))
                {
                    continue;
                }
                if let Ok(commit) = reference.peel_to_commit() {
                    let oid = commit.id();
                    if pushed.insert(oid) {
                        walk.push(oid)?;
                    }
                }
            }
        } else {
            for spec in &filter.refs {
                let oid = resolve_commit(repo, spec)?.id();
                if pushed.insert(oid) {
                    walk.push(oid)?;
                }
            }
        }
        if pushed.is_empty() {
            return Ok((Vec::new(), None));
        }

        let text = filter
            .text
            .as_deref()
            .filter(|t| !t.is_empty())
            .map(str::to_lowercase);
        let author = filter
            .author
            .as_deref()
            .filter(|a| !a.is_empty())
            .map(str::to_lowercase);
        let mut follow_path = if filter.follow {
            filter.path.clone()
        } else {
            None
        };

        let mut commits: Vec<CommitInfo> = Vec::new();
        let mut cursor_seen = after.is_none();
        let mut skipped = 0usize;
        let mut next_cursor = None;
        // Pickaxe budget: a patch scan per candidate commit is ~2.6ms; cap
        // the examined count so huge-repo queries stay responsive (the FE
        // keeps streaming pages, so the budget applies per page request).
        let mut pickaxe_examined = 0usize;

        for oid in walk.by_ref() {
            let oid = oid?;
            if !cursor_seen {
                let hex = oid.to_string();
                let matched = after
                    .map(|a| hex == a || hex.starts_with(a))
                    .unwrap_or(false);
                if matched {
                    cursor_seen = true;
                    continue;
                }
                skipped += 1;
                if skipped > LOG_CURSOR_SCAN_LIMIT {
                    return Err(EngineError::Invalid(format!(
                        "log cursor `{}` not found within {LOG_CURSOR_SCAN_LIMIT} commits",
                        after.unwrap_or_default()
                    )));
                }
                continue;
            }

            let commit = repo.find_commit(oid)?;

            // Pickaxe (-S / -G): the commit's patch must add or remove the
            // needle (-S) AND match the regex (-G) — git requires both when
            // they are given together, so do we. One scan budget covers
            // both: each candidate's patch is examined at most once per page.
            let pickaxe_s = filter.pickaxe.as_deref().filter(|p| !p.is_empty());
            if pickaxe_s.is_some() || pickaxe_regex.is_some() {
                if pickaxe_examined >= LOG_CURSOR_SCAN_LIMIT {
                    // Budget exhausted: the caller resumes from the last
                    // examined commit, so the stream continues in a fresh
                    // page instead of silently dropping the rest of the
                    // walk. The cursor advances every page, so this stays
                    // finite.
                    next_cursor = Some(commit.id().to_string());
                    break;
                }
                pickaxe_examined += 1;
                if let Some(needle) = pickaxe_s {
                    if !commit_touches_string(repo, &commit, needle)? {
                        continue;
                    }
                }
                if let Some(re) = pickaxe_regex.as_ref() {
                    if !commit_touches_regex(repo, &commit, re)? {
                        continue;
                    }
                }
            }

            if let Some(path) = &filter.path {
                let touched = match &mut follow_path {
                    Some(current) => commit_follows_path(repo, &commit, current),
                    None => commit_touches_path(repo, &commit, path)?,
                };
                if !touched {
                    continue;
                }
            }

            if let Some(after_unix) = filter.after_unix {
                if commit.committer().when().seconds() < after_unix {
                    continue;
                }
            }
            if let Some(before_unix) = filter.before_unix {
                if commit.committer().when().seconds() > before_unix {
                    continue;
                }
            }

            if let Some(author_filter) = &author {
                let sig = commit.author();
                let haystack = format!(
                    "{} <{}>",
                    sig.name().unwrap_or_default(),
                    sig.email().unwrap_or_default()
                )
                .to_lowercase();
                if !haystack.contains(author_filter.as_str()) {
                    continue;
                }
            }

            if let Some(re) = &text_regex {
                let message = commit.message().unwrap_or_default();
                let summary = commit.summary().ok().flatten().unwrap_or_default();
                if !re.is_match(message) && !re.is_match(summary) {
                    continue;
                }
            } else if let Some(text_filter) = &text {
                let message = commit.message().unwrap_or_default().to_lowercase();
                let summary = commit
                    .summary()
                    .ok()
                    .flatten()
                    .unwrap_or_default()
                    .to_lowercase();
                if !message.contains(text_filter.as_str())
                    && !summary.contains(text_filter.as_str())
                {
                    continue;
                }
            }

            let sha = commit.id().to_string();
            commits.push(commit_info(&commit, &decos));
            if commits.len() >= limit {
                next_cursor = Some(sha);
                break;
            }
        }

        Ok((commits, next_cursor))
    }

    fn blame(
        &self,
        repo: &Repository,
        path: &str,
        from: Option<&str>,
    ) -> EngineResult<Vec<BlameLine>> {
        let mut opts = BlameOptions::new();
        let target_commit = match from {
            Some(spec) => {
                let commit = resolve_commit(repo, spec)?;
                opts.newest_commit(commit.id());
                Some(commit)
            }
            None => {
                // Default target is HEAD (contract: "from commit-ish or HEAD").
                // On an unborn HEAD there is nothing to blame yet.
                match repo.head() {
                    Ok(head) => match head.peel_to_commit() {
                        Ok(commit) => {
                            opts.newest_commit(commit.id());
                            Some(commit)
                        }
                        Err(_) => None,
                    },
                    Err(_) => None,
                }
            }
        };

        let blame = repo.blame_file(Path::new(path), Some(&mut opts))?;

        // Blame maps hunks onto the *final* file; read that file's lines.
        let content: Vec<u8> = match &target_commit {
            Some(commit) => {
                let tree = tree_of(commit)?;
                let entry = tree.get_path(Path::new(path)).map_err(|e| {
                    EngineError::Invalid(format!(
                        "`{path}` not found in `{}`: {e}",
                        from.unwrap_or_default()
                    ))
                })?;
                let blob = entry
                    .to_object(repo)?
                    .peel_to_blob()
                    .map_err(|e| EngineError::Invalid(format!("`{path}` is not a blob: {e}")))?;
                blob.content().to_vec()
            }
            None => {
                let workdir = repo
                    .workdir()
                    .ok_or_else(|| EngineError::Invalid("bare repository has no workdir".into()))?;
                std::fs::read(workdir.join(path))?
            }
        };
        let lines: Vec<String> = String::from_utf8_lossy(&content)
            .split('\n')
            .map(|l| l.trim_end_matches('\r').to_owned())
            .collect();
        // `split('\n')` yields a trailing empty element for a newline-ended
        // file; drop it so line numbers line up.
        let lines = if content.ends_with(b"\n") {
            &lines[..lines.len() - 1]
        } else {
            &lines[..]
        };

        let mut out = Vec::new();
        for hunk in blame.iter() {
            for i in 0..hunk.lines_in_hunk() {
                let line_no = (hunk.final_start_line() + i) as u32;
                let text = lines
                    .get(line_no.saturating_sub(1) as usize)
                    .cloned()
                    .unwrap_or_default();
                out.push(BlameLine {
                    line_no,
                    sha: hunk.orig_commit_id().to_string(),
                    signature: sig_opt_to_model(hunk.orig_signature()),
                    final_sha: hunk.final_commit_id().to_string(),
                    final_signature: sig_opt_to_model(hunk.final_signature()),
                    text,
                });
            }
        }
        out.sort_by_key(|l| l.line_no);
        Ok(out)
    }

    fn refs(&self, repo: &Repository) -> EngineResult<Vec<(String, String)>> {
        let mut out = Vec::new();
        for reference in repo.references()?.flatten() {
            let Ok(name) = reference.name() else {
                continue;
            };
            let Some(short) = short_ref_name(name) else {
                continue;
            };
            if let Ok(commit) = reference.peel_to_commit() {
                out.push((short.to_owned(), commit.id().to_string()));
            }
        }
        out.sort();
        Ok(out)
    }

    // ---------- M2: mutations — forwarded to engine/{mutations,branches,netops}.rs ----------

    fn stage(&self, repo: &Repository, req: &StageRequest) -> EngineResult<()> {
        self.stage_impl(repo, req)
    }

    fn stage_all(&self, repo: &Repository, unstage: bool) -> EngineResult<()> {
        self.stage_all_impl(repo, unstage)
    }

    fn discard(&self, repo: &Repository, targets: &[StageTarget]) -> EngineResult<()> {
        self.discard_impl(repo, targets)
    }

    fn commit(&self, repo: &Repository, opts: &CommitOptions) -> EngineResult<String> {
        self.commit_impl(repo, opts)
    }

    fn signing_info(&self, repo: &Repository) -> EngineResult<SigningInfo> {
        self.signing_info_impl(repo)
    }

    fn hooks(&self, repo: &Repository) -> EngineResult<Vec<HookInfo>> {
        self.hooks_impl(repo)
    }

    fn branches(&self, repo: &Repository) -> EngineResult<Vec<BranchInfo>> {
        self.branches_impl(repo)
    }

    fn branch_create(
        &self,
        repo: &Repository,
        name: &str,
        from: Option<&str>,
        checkout: bool,
    ) -> EngineResult<()> {
        self.branch_create_impl(repo, name, from, checkout)
    }

    fn branch_switch(&self, repo: &Repository, name: &str, force: bool) -> EngineResult<()> {
        self.branch_switch_impl(repo, name, force)
    }

    fn branch_is_merged(&self, repo: &Repository, name: &str, into: &str) -> EngineResult<bool> {
        self.branch_is_merged_impl(repo, name, into)
    }

    fn branch_delete(&self, repo: &Repository, name: &str, force: bool) -> EngineResult<()> {
        self.branch_delete_impl(repo, name, force)
    }

    fn branch_rename(&self, repo: &Repository, old: &str, new: &str) -> EngineResult<()> {
        self.branch_rename_impl(repo, old, new)
    }

    fn tag_create(
        &self,
        repo: &Repository,
        name: &str,
        target: Option<&str>,
        message: Option<&str>,
    ) -> EngineResult<()> {
        self.tag_create_impl(repo, name, target, message)
    }

    fn tag_delete(&self, repo: &Repository, name: &str) -> EngineResult<()> {
        self.tag_delete_impl(repo, name)
    }

    fn tag_list(&self, repo: &Repository) -> EngineResult<Vec<TagInfo>> {
        self.tags_impl(repo)
    }

    fn remote_branches(&self, repo: &Repository) -> EngineResult<Vec<RemoteBranchInfo>> {
        self.remote_branches_impl(repo)
    }

    fn branch_checkout_remote(
        &self,
        repo: &Repository,
        remote: &str,
        name: &str,
        new_local: Option<&str>,
    ) -> EngineResult<String> {
        self.branch_checkout_remote_impl(repo, remote, name, new_local)
    }

    fn remotes(&self, repo: &Repository) -> EngineResult<Vec<RemoteInfo>> {
        self.remotes_impl(repo)
    }

    fn remote_add(&self, repo: &Repository, name: &str, url: &str) -> EngineResult<()> {
        self.remote_add_impl(repo, name, url)
    }

    fn remote_remove(&self, repo: &Repository, name: &str) -> EngineResult<()> {
        self.remote_remove_impl(repo, name)
    }

    fn remote_set_url(
        &self,
        repo: &Repository,
        name: &str,
        url: &str,
        push: bool,
    ) -> EngineResult<()> {
        self.remote_set_url_impl(repo, name, url, push)
    }

    fn fetch(
        &self,
        repo: &Repository,
        opts: &FetchOptions,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        self.fetch_impl(repo, opts, progress)
    }

    fn pull(
        &self,
        repo: &Repository,
        opts: &PullOptions,
        progress: &mut dyn FnMut(FetchProgress),
    ) -> EngineResult<NetStats> {
        self.pull_impl(repo, opts, progress)
    }

    fn push(
        &self,
        repo: &Repository,
        opts: &PushOptions,
        progress: &mut dyn FnMut(PushProgress),
    ) -> EngineResult<NetStats> {
        self.push_impl(repo, opts, progress)
    }
}

// ---------------------------------------------------------------------------
// M12 tests: pickaxe -G (this file's other M12 work lives in signing.rs /
// bisect.rs so lanes never share an impl block).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod m12_pickaxe_tests {
    use std::path::{Path, PathBuf};

    use git2::{IndexAddOption, Repository, RepositoryInitOptions};

    use super::*;

    const ENGINE: Libgit2Engine = Libgit2Engine;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let dir =
                std::env::temp_dir().join(format!("mygitui-pickaxe-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            TempDir(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let dir = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }

    struct Fixture {
        repo: Repository,
        _dir: Option<TempDir>,
    }

    fn init_fixture(name: &str) -> Fixture {
        let dir = TempDir::new(name);
        let mut opts = RepositoryInitOptions::new();
        opts.bare(false).initial_head("main");
        let repo = Repository::init_opts(dir.path(), &opts).expect("init temp repo");
        repo.config()
            .and_then(|mut c| {
                c.set_str("user.name", "Pickaxe Test")?;
                c.set_str("user.email", "pickaxe@test.local")?;
                c.set_bool("core.autocrlf", false)
            })
            .expect("configure identity");
        Fixture {
            repo,
            _dir: Some(dir),
        }
    }

    fn commit_file(repo: &Repository, path: &str, content: &str, message: &str) -> String {
        let file = repo.workdir().unwrap().join(path);
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(file, content).expect("write file");
        let mut index = repo.index().expect("index");
        index
            .add_all(["*"].iter(), IndexAddOption::DEFAULT, None)
            .expect("add all");
        index.write().expect("write index");
        let tree_oid = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_oid).expect("tree");
        let sig = repo.signature().expect("signature");
        let mut parents = Vec::new();
        if let Ok(head) = repo.head() {
            parents.push(head.peel_to_commit().expect("head commit"));
        }
        let parent_refs: Vec<&git2::Commit<'_>> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parent_refs)
            .expect("commit")
            .to_string()
    }

    /// c1 base; c2 adds "alpha NEEDLE" to f.txt; c3 adds "NEEDLE" to a new
    /// g.txt (patch has no "alpha"); c4 edits the "alpha NEEDLE" line away
    /// (removes the old line, adds a new one).
    fn token_fixture(name: &str) -> (Fixture, [String; 4]) {
        let fx = init_fixture(name);
        let c1 = commit_file(&fx.repo, "f.txt", "one\ntwo\n", "c1 base");
        let c2 = commit_file(
            &fx.repo,
            "f.txt",
            "one\ntwo\nalpha NEEDLE\n",
            "c2 add token",
        );
        let c3 = commit_file(&fx.repo, "g.txt", "NEEDLE only\n", "c3 other file");
        let c4 = commit_file(
            &fx.repo,
            "f.txt",
            "one\ntwo\nalpha NEEDLE v2\n",
            "c4 edit token",
        );
        (fx, [c1, c2, c3, c4])
    }

    fn shas(commits: &[CommitInfo]) -> Vec<String> {
        commits.iter().map(|c| c.sha.clone()).collect()
    }

    #[test]
    fn pickaxe_regex_matches_only_patch_matching_commits() {
        let (fx, [c1, c2, _c3, c4]) = token_fixture("regex");

        // -G "alpha NEEDLE$": c2 added the line, c4 removed it; c3's patch
        // (g.txt) has no "alpha".
        let filter = LogFilter {
            pickaxe_regex: Some("(?m)alpha NEEDLE$".into()),
            ..Default::default()
        };
        let (commits, _) = ENGINE.log(&fx.repo, &filter, 100, None).expect("log");
        assert_eq!(
            shas(&commits),
            [c4.as_str(), c2.as_str()],
            "-G matches c2 and c4"
        );

        // The base commit touches nothing matching.
        let (commits, _) = ENGINE
            .log(
                &fx.repo,
                &LogFilter {
                    pickaxe_regex: Some("alpha".into()),
                    ..Default::default()
                },
                100,
                None,
            )
            .expect("log");
        assert_eq!(shas(&commits), [c4.as_str(), c2.as_str()]);
        let _ = c1;
    }

    #[test]
    fn pickaxe_regex_and_substring_intersect() {
        let (fx, [c1, c2, c3, c4]) = token_fixture("intersect");

        // -S alone: every commit whose patch adds/removes NEEDLE (c2, c3, c4).
        let (commits, _) = ENGINE
            .log(
                &fx.repo,
                &LogFilter {
                    pickaxe: Some("NEEDLE".into()),
                    ..Default::default()
                },
                100,
                None,
            )
            .expect("log");
        assert_eq!(shas(&commits), [c4.as_str(), c3.as_str(), c2.as_str()]);

        // -S "NEEDLE" + -G "alpha" (both set: git requires both) = c2 + c4;
        // c3's NEEDLE patch never mentions alpha.
        let (commits, _) = ENGINE
            .log(
                &fx.repo,
                &LogFilter {
                    pickaxe: Some("NEEDLE".into()),
                    pickaxe_regex: Some("alpha".into()),
                    ..Default::default()
                },
                100,
                None,
            )
            .expect("log");
        assert_eq!(
            shas(&commits),
            [c4.as_str(), c2.as_str()],
            "S+G intersection"
        );
        let _ = (c1, c3);
    }

    #[test]
    fn pickaxe_regex_invalid_pattern_fails_fast() {
        let (fx, _) = token_fixture("invalid");
        let filter = LogFilter {
            pickaxe_regex: Some("alpha(".into()),
            ..Default::default()
        };
        match ENGINE.log(&fx.repo, &filter, 10, None) {
            Err(EngineError::Invalid(msg)) => {
                assert!(msg.contains("invalid pickaxe regex"), "{msg}")
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn pickaxe_regex_empty_pattern_is_ignored() {
        let (fx, [c1, c2, c3, c4]) = token_fixture("empty");
        let (commits, _) = ENGINE
            .log(
                &fx.repo,
                &LogFilter {
                    pickaxe_regex: Some(String::new()),
                    ..Default::default()
                },
                100,
                None,
            )
            .expect("log");
        assert_eq!(
            shas(&commits),
            [c4.as_str(), c3.as_str(), c2.as_str(), c1.as_str()],
            "no filter applied"
        );
    }
}
