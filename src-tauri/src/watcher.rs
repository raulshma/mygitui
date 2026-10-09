//! Recursive filesystem watcher for one open repository's workdir.
//!
//! Wraps `notify::recommended_watcher` behind a debounce thread: events are
//! collected and collapsed until the filesystem has been quiet for
//! [`DEBOUNCE`] (200 ms), then one batch is emitted. Batches are pre-filtered
//! so clients never see noise: git object-store churn is dropped, only the
//! `.git` metadata entries that can change HEAD/status pass through, and
//! M12 adds real gitignore matching — a `Gitignore` matcher is built per
//! repo from the root `.gitignore`, nested `.gitignore` files (up to depth
//! 3), `.git/info/exclude`, and the global `core.excludesFile`, rebuilt
//! lazily whenever those sources change (mtime check per window + forced
//! rebuild when a `.gitignore` event itself arrives).
//!
//! Robustness: a watcher error logs a warning, emits a `full = true` batch so
//! clients resync everything, and restarts the watcher with backoff (3 tries
//! per failure episode, resetting on success); after the budget is spent the
//! thread exits ("dead") until the repo is reopened.
//!
//! The emitter is injected as a callback so the module has no tauri
//! dependency and is unit-testable; `repo` wires the callback to the
//! `repo-changed` tauri event (payload: [`RepoChangedEvent`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher as _};
use serde::Serialize;

use crate::engine::types::RepoId;

/// Debounce window: a batch is emitted after this much FS quiet time.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Tauri event name for emitted batches (registry: docs/contracts.md).
pub const REPO_CHANGED_EVENT: &str = "repo-changed";

/// Backoff schedule for watcher restarts; the thread dies after these are
/// exhausted without a successful restart (the repo must be reopened).
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_millis(200),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// Payload of the `repo-changed` event (registry: docs/contracts.md).
///
/// JSON shape (exact): `{"repo_id": string, "paths": string[], "head_moved": bool, "full": bool}`.
#[derive(Debug, Clone, Serialize)]
pub struct RepoChangedEvent {
    pub repo_id: RepoId,
    /// Repo-relative paths, forward slashes, deduplicated, sorted.
    pub paths: Vec<String>,
    /// True when any path was `.git/HEAD` or under `.git/refs` (clients
    /// refresh refs/log); linked-worktree HEAD files count too.
    pub head_moved: bool,
    /// True when the watcher errored/restarted: resync everything.
    pub full: bool,
}

/// Workdir-relative path components never worth reporting (dependency and
/// build output directories; heuristic until real ignore parsing lands).
pub const IGNORED_DIRS: [&str; 5] = ["node_modules", "target", ".next", "dist", "build"];

/// Entries directly under `.git/` that DO matter (HEAD moves, index writes,
/// ref updates, linked worktrees, in-progress rebase/cherry-pick state).
/// Everything else in `.git` (objects, packs, hooks, locks, logs, ...) is
/// ignored.
pub const ALLOWED_GIT_ENTRIES: [&str; 8] = [
    "HEAD",
    "index",
    "refs",
    "worktrees",
    "MERGE_HEAD",
    "rebase-merge",
    "rebase-apply",
    "sequencer",
];

/// Whether `name` is one of the watchable entries directly under `.git/`.
pub fn is_git_entry_allowed(name: &str) -> bool {
    ALLOWED_GIT_ENTRIES.contains(&name)
}

/// Baseline ignore filter on a repo-relative, forward-slash path.
///
/// This is the part that needs no repo context:
/// - any component in [`IGNORED_DIRS`] → ignored;
/// - a `.git` component with no successor (the directory itself) → ignored;
/// - the component right after `.git` not in [`ALLOWED_GIT_ENTRIES`] →
///   ignored (deep `.git` internals; lock files such as `index.lock` are
///   dropped because only exact allowed names pass).
///
/// Per-repo gitignore matching (root/nested `.gitignore`, excludes, global
/// excludesFile) lives in [`IgnoreCache`] and runs after this check in the
/// event loop.
pub fn is_ignored_rel(rel: &str) -> bool {
    let comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
    for comp in &comps {
        if IGNORED_DIRS.contains(comp) {
            return true;
        }
    }
    for (i, comp) in comps.iter().enumerate() {
        if *comp == ".git" {
            match comps.get(i + 1) {
                // The `.git` entry itself carries no signal.
                None => return true,
                Some(next) if !is_git_entry_allowed(next) => return true,
                Some(_) => {}
            }
        }
    }
    false
}

/// Whether a repo-relative path means HEAD (or a ref) may have moved.
pub fn is_head_move_rel(rel: &str) -> bool {
    rel == ".git/HEAD"
        || rel == ".git/refs"
        || rel.starts_with(".git/refs/")
        || (rel.starts_with(".git/worktrees/") && rel.ends_with("/HEAD"))
}

// ---------------------------------------------------------------------------
// M12: gitignore-aware filtering
// ---------------------------------------------------------------------------

/// How deep below the repo root nested `.gitignore` files are collected
/// (root is depth 0; depth 3 = `a/b/c/.gitignore`).
const GITIGNORE_MAX_DEPTH: u8 = 3;

/// Hard cap on directories scanned while collecting nested `.gitignore`
/// files (huge source trees must not stall the debounce thread).
const GITIGNORE_SCAN_CAP: usize = 2000;

/// Repo-relative paths whose change means the ignore matcher must be rebuilt
/// right away (the source itself may be new, so mtimes alone can't catch it).
fn is_ignore_source_rel(rel: &str) -> bool {
    rel == ".gitignore" || rel.ends_with("/.gitignore") || rel == ".git/info/exclude"
}

/// The repo's git directory: `root/.git` when it is a directory, or the
/// target of the `gitdir: <path>` file for linked worktrees.
fn git_dir_of(root: &Path) -> Option<PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let text = std::fs::read_to_string(&dot_git).ok()?;
    let target = text.trim().strip_prefix("gitdir:")?.trim();
    let path = PathBuf::from(target);
    Some(if path.is_absolute() {
        path
    } else {
        root.join(path)
    })
}

/// Global `core.excludesFile`, when configured and present.
fn global_excludes_file() -> Option<PathBuf> {
    git2::Config::open_default()
        .and_then(|cfg| cfg.get_path("core.excludesFile"))
        .ok()
}

/// Nested `.gitignore` files from the root down to [`GITIGNORE_MAX_DEPTH`].
/// `.git` internals and known dependency/build dirs are not descended into;
/// the walk is capped at [`GITIGNORE_SCAN_CAP`] directories.
fn nested_gitignores(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0u8)];
    let mut scanned = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        if scanned >= GITIGNORE_SCAN_CAP {
            break;
        }
        scanned += 1;
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if file_type.is_dir() {
                if name == ".git" || IGNORED_DIRS.contains(&name.as_ref()) {
                    continue;
                }
                if depth < GITIGNORE_MAX_DEPTH {
                    stack.push((entry.path(), depth + 1));
                }
            } else if name == ".gitignore" {
                found.push(entry.path());
            }
        }
    }
    found
}

/// Pure gitignore decision (free fn so tests can build matchers without a
/// repo): drop when the path OR any parent matches an `Ignore` pattern —
/// a plain `matched` call would miss `build/x` for a `build/` dir pattern,
/// which git expresses by not descending at all.
fn ignore_matches(gi: &Gitignore, rel: &str, is_dir: bool) -> bool {
    matches!(
        gi.matched_path_or_any_parents(rel, is_dir),
        ignore::Match::Ignore(_)
    )
}

/// Mtime-tracked gitignore matcher for one repo root. Built from the root
/// `.gitignore`, nested `.gitignore` files (to [`GITIGNORE_MAX_DEPTH`]),
/// `.git/info/exclude`, and the global `core.excludesFile` — plus a baseline
/// pattern list for dependency/build dirs git itself would never ignore.
/// Rebuilt lazily when any source's mtime changed, or forced when a source
/// file is itself an event (it may be brand new).
struct IgnoreCache {
    root: PathBuf,
    gi: Gitignore,
    /// Every file the matcher was built from + its mtime at build time.
    sources: Vec<(PathBuf, Option<SystemTime>)>,
}

impl IgnoreCache {
    fn build(root: &Path) -> IgnoreCache {
        let mut builder = GitignoreBuilder::new(root);
        // Baseline: `node_modules/` etc. are noise in every repo, ignored or
        // not — git has no default ignore for them.
        for dir in IGNORED_DIRS {
            let _ = builder.add_line(None, &format!("{dir}/"));
        }
        let mut files = nested_gitignores(root);
        if let Some(exclude) = git_dir_of(root)
            .map(|git_dir| git_dir.join("info/exclude"))
            .filter(|p| p.is_file())
        {
            files.push(exclude);
        }
        if let Some(global) = global_excludes_file().filter(|p| p.is_file()) {
            files.push(global);
        }
        // Shallowest first (root .gitignore), so deeper — more specific —
        // files are added later and win ties, mirroring git's precedence.
        files.sort_by_key(|p| (p.components().count(), p.to_path_buf()));
        let mut sources = Vec::with_capacity(files.len());
        for path in files {
            sources.push((path.clone(), mtime_of(&path)));
            // Unreadable/undecodable ignore files are skipped: filtering is
            // best-effort noise reduction, never a hard failure.
            let _ = builder.add(&path);
        }
        let gi = builder.build().unwrap_or_else(|_| Gitignore::empty());
        IgnoreCache {
            root: root.to_path_buf(),
            gi,
            sources,
        }
    }

    /// Rebuild when any tracked source file's mtime changed (added, edited,
    /// or deleted). Cheap: a handful of stats per debounce window.
    fn refresh_if_changed(&mut self) {
        for (path, seen) in &self.sources {
            if mtime_of(path) != *seen {
                self.rebuild();
                return;
            }
        }
    }

    /// Unconditional rebuild — used when an ignore source file itself shows
    /// up as an event (it may be new and therefore absent from `sources`).
    fn force_rebuild(&mut self) {
        self.rebuild();
    }

    fn rebuild(&mut self) {
        *self = IgnoreCache::build(&self.root);
    }

    /// Whether a repo-relative path is ignored (parents included).
    fn ignores(&self, rel: &str, is_dir: bool) -> bool {
        ignore_matches(&self.gi, rel, is_dir)
    }
}

fn mtime_of(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Strip a Windows extended-length (`\\?\`) drive prefix, if present.
/// Verbatim UNC paths (`\\?\UNC\...`) are passed through unchanged.
pub fn strip_verbatim(p: &Path) -> PathBuf {
    let s = p.as_os_str().to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p.to_path_buf(),
    }
}

/// Repo-relative, forward-slash rendering of an absolute event path.
/// Paths outside `root` (or with unmatchable prefixes) are returned as-is.
pub fn relativize(root: &Path, path: &Path) -> String {
    let root = strip_verbatim(root);
    let path = strip_verbatim(path);
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Messages into the debounce thread.
enum Feed {
    Event(Event),
    Error(notify::Error),
    Shutdown,
}

/// Handle to a running watcher; dropping it shuts the watcher down (the
/// debounce thread is joined in [`Drop`]).
pub struct RepoWatcher {
    tx: Sender<Feed>,
    thread: Option<JoinHandle<()>>,
}

impl RepoWatcher {
    /// Watch `root` recursively and call `on_batch` for every debounced,
    /// filtered batch. `generation` is bumped (by one) on every emitted batch
    /// so stream consumers can detect staleness.
    pub fn start(
        repo_id: RepoId,
        root: PathBuf,
        generation: Arc<AtomicU64>,
        on_batch: impl Fn(RepoChangedEvent) + Send + 'static,
    ) -> RepoWatcher {
        let (tx, rx) = mpsc::channel::<Feed>();
        let thread = thread::Builder::new()
            .name(format!("watcher-{}", repo_id.0))
            .spawn({
                let tx = tx.clone();
                move || {
                    run(rx, tx, repo_id, root, generation, Box::new(on_batch));
                }
            })
            .expect("watcher thread spawn");
        RepoWatcher {
            tx,
            thread: Some(thread),
        }
    }
}

impl Drop for RepoWatcher {
    fn drop(&mut self) {
        let _ = self.tx.send(Feed::Shutdown);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

/// Everything the debounce thread needs to emit a batch.
struct BatchCtx {
    repo_id: RepoId,
    root: PathBuf,
    generation: Arc<AtomicU64>,
    on_batch: Box<dyn Fn(RepoChangedEvent) + Send>,
}

/// Create the notify watcher for `root`, hooking its callback to `tx`.
/// Returns `None` on creation or initial-watch failure (already warned).
fn create_watcher(root: &Path, tx: &Sender<Feed>) -> Option<RecommendedWatcher> {
    let callback = {
        let tx = tx.clone();
        move |res: Result<Event, notify::Error>| {
            let feed = match res {
                Ok(event) => Feed::Event(event),
                Err(err) => Feed::Error(err),
            };
            // Ignored when the debounce thread has exited: the watcher is
            // dropped alongside it, so this fires at most a few more times.
            let _ = tx.send(feed);
        }
    };
    let mut watcher = notify::recommended_watcher(callback).ok()?;
    match watcher.watch(root, RecursiveMode::Recursive) {
        Ok(()) => Some(watcher),
        Err(err) => {
            tracing::warn!(root = %root.display(), error = %err, "watcher: initial watch failed");
            None
        }
    }
}

/// Try to (re)create the notify watcher, sleeping through the backoff
/// schedule. `attempts` counts consecutive failures; a success resets it.
/// Returns `None` when the budget is spent (thread goes dead) — a final
/// `full = true` event is emitted so clients resync.
fn restart(ctx: &BatchCtx, tx: &Sender<Feed>, attempts: &mut usize) -> Option<RecommendedWatcher> {
    while *attempts < RETRY_BACKOFF.len() {
        let wait = RETRY_BACKOFF[*attempts];
        *attempts += 1;
        thread::sleep(wait);
        tracing::warn!(
            root = %ctx.root.display(),
            attempt = *attempts,
            "watcher: restarting with backoff"
        );
        if let Some(watcher) = create_watcher(&ctx.root, tx) {
            *attempts = 0;
            return Some(watcher);
        }
    }
    tracing::error!(
        root = %ctx.root.display(),
        tries = RETRY_BACKOFF.len(),
        "watcher: dead after exhausted restarts; reopen the repo to retry"
    );
    emit(ctx, Vec::new(), false, true);
    None
}

/// Sort, dedupe, bump the generation, and deliver one batch.
fn emit(ctx: &BatchCtx, mut paths: Vec<String>, head_moved: bool, full: bool) {
    paths.sort();
    paths.dedup();
    ctx.generation.fetch_add(1, Ordering::Relaxed);
    (ctx.on_batch)(RepoChangedEvent {
        repo_id: ctx.repo_id.clone(),
        paths,
        head_moved,
        full,
    });
}

/// Debounce-thread main loop.
fn run(
    rx: Receiver<Feed>,
    tx: Sender<Feed>,
    repo_id: RepoId,
    root: PathBuf,
    generation: Arc<AtomicU64>,
    on_batch: Box<dyn Fn(RepoChangedEvent) + Send>,
) {
    let ctx = BatchCtx {
        repo_id,
        root,
        generation,
        on_batch,
    };

    // M12: gitignore-aware filtering. Built once per watcher lifetime and
    // refreshed lazily (source mtimes) or forced (a .gitignore event).
    let mut ignores = IgnoreCache::build(&ctx.root);

    let mut attempts = 0usize;
    let mut watcher = create_watcher(&ctx.root, &tx);
    if watcher.is_none() {
        watcher = restart(&ctx, &tx, &mut attempts);
    }

    'events: loop {
        // Re-check the ignore sources once per debounce window.
        ignores.refresh_if_changed();

        let mut pending: Vec<String> = Vec::new();
        let mut head_moved = false;
        let mut errored = false;

        // Collect one debounce window: the first feed blocks, then we keep
        // folding events until DEBOUNCE passes with no traffic.
        loop {
            let feed = if pending.is_empty() && !errored {
                match rx.recv() {
                    Ok(feed) => feed,
                    Err(_) => return, // every sender dropped: paranoia path
                }
            } else {
                match rx.recv_timeout(DEBOUNCE) {
                    Ok(feed) => feed,
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            };
            match feed {
                Feed::Shutdown => {
                    if !pending.is_empty() || errored {
                        emit(&ctx, std::mem::take(&mut pending), head_moved, errored);
                    }
                    return;
                }
                Feed::Error(err) => {
                    tracing::warn!(root = %ctx.root.display(), error = %err, "watcher error");
                    errored = true;
                }
                Feed::Event(event) => {
                    for path in &event.paths {
                        let rel = relativize(&ctx.root, path);
                        if is_ignored_rel(&rel) {
                            continue;
                        }
                        if is_ignore_source_rel(&rel) {
                            // A (new?) ignore source changed: rebuild before
                            // filtering the rest of the window with it.
                            ignores.force_rebuild();
                        }
                        let is_dir = std::fs::metadata(strip_verbatim(path))
                            .map(|m| m.is_dir())
                            .unwrap_or(false);
                        if ignores.ignores(&rel, is_dir) {
                            continue;
                        }
                        if is_head_move_rel(&rel) {
                            head_moved = true;
                        }
                        pending.push(rel);
                    }
                }
            }
        }

        if pending.is_empty() && !errored {
            continue; // only ignorable events happened in this window
        }
        emit(&ctx, std::mem::take(&mut pending), head_moved, errored);

        if errored {
            // Clients were told to resync (full = true); drop the broken
            // watcher (so it stops feeding events) and restart with backoff.
            drop(watcher.take());
            watcher = restart(&ctx, &tx, &mut attempts);
            if watcher.is_none() {
                break 'events;
            }
        }
    }
    drop(watcher);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn allowed_git_entries_list() {
        for name in [
            "HEAD",
            "index",
            "refs",
            "worktrees",
            "MERGE_HEAD",
            "rebase-merge",
            "rebase-apply",
            "sequencer",
        ] {
            assert!(is_git_entry_allowed(name), "{name} should be allowed");
        }
        for name in [
            "objects",
            "packs",
            "hooks",
            "logs",
            "config",
            "ORIG_HEAD",
            "FETCH_HEAD",
            "index.lock",
            "COMMIT_EDITMSG",
            "shallow",
        ] {
            assert!(!is_git_entry_allowed(name), "{name} should NOT be allowed");
        }
    }

    #[test]
    fn ignore_filter_workdir_paths() {
        assert!(!is_ignored_rel("src/main.py"));
        assert!(!is_ignored_rel("README.md"));
        assert!(!is_ignored_rel("a/b/c.txt"));
        // Dependency/build directories at any depth.
        assert!(is_ignored_rel("node_modules/react/index.js"));
        assert!(is_ignored_rel("crates/target/debug/x"));
        assert!(is_ignored_rel("web/.next/static/x.js"));
        assert!(is_ignored_rel("dist/bundle.js"));
        assert!(is_ignored_rel("build/out.obj"));
        assert!(is_ignored_rel("apps/web/node_modules/x"));
    }

    #[test]
    fn ignore_filter_git_internals() {
        // Watchable .git metadata.
        assert!(!is_ignored_rel(".git/HEAD"));
        assert!(!is_ignored_rel(".git/index"));
        assert!(!is_ignored_rel(".git/refs/heads/main"));
        assert!(!is_ignored_rel(".git/worktrees/feature/HEAD"));
        assert!(!is_ignored_rel(".git/MERGE_HEAD"));
        assert!(!is_ignored_rel(".git/rebase-merge/done"));
        assert!(!is_ignored_rel(".git/rebase-apply/0001"));
        assert!(!is_ignored_rel(".git/sequencer/todo"));
        // Deep internals and locks are noise.
        assert!(is_ignored_rel(".git"));
        assert!(is_ignored_rel(".git/objects/ab/cdef123"));
        assert!(is_ignored_rel(".git/objects/pack/pack-1.pack"));
        assert!(is_ignored_rel(".git/index.lock"));
        assert!(is_ignored_rel(".git/hooks/pre-commit"));
        assert!(is_ignored_rel(".git/logs/HEAD"));
        assert!(is_ignored_rel(".git/ORIG_HEAD"));
        assert!(is_ignored_rel(".git/config"));
        assert!(is_ignored_rel(".git/COMMIT_EDITMSG"));
    }

    #[test]
    fn head_move_detection() {
        assert!(is_head_move_rel(".git/HEAD"));
        assert!(is_head_move_rel(".git/refs"));
        assert!(is_head_move_rel(".git/refs/heads/main"));
        assert!(is_head_move_rel(".git/refs/tags/v1.0.0"));
        assert!(is_head_move_rel(".git/worktrees/feature/HEAD"));
        assert!(is_head_move_rel(".git/refs/remotes/origin/main"));
        assert!(!is_head_move_rel(".git/index"));
        assert!(!is_head_move_rel(".git/MERGE_HEAD"));
        assert!(!is_head_move_rel("src/main.py"));
    }

    #[test]
    fn relativize_uses_forward_slashes() {
        // Forward slashes segment on every platform, so this case is portable.
        let root = Path::new("C:/Code/mygitui/fixtures/basic");
        let path = Path::new("C:/Code/mygitui/fixtures/basic/src/main.py");
        assert_eq!(relativize(root, path), "src/main.py");
    }

    // Backslash separators and \\?\ verbatim prefixes only segment as path
    // components on Windows; on unix they'd be literal filename characters.
    #[cfg(windows)]
    #[test]
    fn relativize_normalizes_windows_separators_and_verbatim_prefix() {
        let root_bs = Path::new(r"C:\Code\mygitui\fixtures\basic\");
        let path_bs = Path::new(r"C:\Code\mygitui\fixtures\basic\src\main.py");
        assert_eq!(relativize(root_bs, path_bs), "src/main.py");

        let root = Path::new(r"C:\Code\mygitui");
        let path = Path::new(r"\\?\C:\Code\mygitui\src\lib.rs");
        assert_eq!(relativize(root, path), "src/lib.rs");
    }

    #[test]
    fn relativize_outside_root_falls_back_to_full_path() {
        let root = Path::new("C:/Code/mygitui/fixtures/basic");
        let path = Path::new("C:/Code/mygitui/README.md");
        assert_eq!(relativize(root, path), "C:/Code/mygitui/README.md");
    }

    /// Integration-style: needs a real FS and the OS watcher backend.
    /// Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "integration: real FS watcher backend; run with `cargo test -- --ignored`"]
    fn watcher_emits_batch_for_new_file() {
        let root =
            std::env::temp_dir().join(format!("mygitui-watcher-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("create temp dir");
        git2::Repository::init(&root).expect("init temp repo");

        let (batch_tx, batch_rx) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let watcher = RepoWatcher::start(
            RepoId("watcher-test".into()),
            root.clone(),
            generation.clone(),
            move |batch| {
                let _ = batch_tx.send(batch);
            },
        );

        // Give the OS watcher a moment to arm before touching the tree.
        thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("src/new.py"), b"print('hi')\n").expect("write file");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match batch_rx.recv_timeout(remaining) {
                Ok(batch) => {
                    if batch.paths.iter().any(|p| p == "src/new.py") {
                        assert!(!batch.full, "plain file write must not be a full resync");
                        assert!(!batch.head_moved);
                        found = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        drop(watcher); // joins the debounce thread
        assert!(found, "expected a batch containing src/new.py within 5s");
        assert!(
            generation.load(Ordering::Relaxed) >= 1,
            "generation must bump"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // -------------------------------------------------------------------
    // M12: gitignore-aware filtering
    // -------------------------------------------------------------------

    struct TempRepo(PathBuf);

    impl TempRepo {
        fn new(tag: &str) -> TempRepo {
            let dir =
                std::env::temp_dir().join(format!("mygitui-wtignore-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create temp dir");
            git2::Repository::init(&dir).expect("init temp repo");
            TempRepo(dir)
        }

        fn write(&self, rel: &str, content: &str) {
            let path = self.0.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(path, content).expect("write file");
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let dir = self.0.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(50));
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }

    fn matcher_from_lines(lines: &[&str]) -> Gitignore {
        let mut builder = GitignoreBuilder::new("");
        for line in lines {
            builder.add_line(None, line).expect("add_line");
        }
        builder.build().expect("build")
    }

    #[test]
    fn gitignore_match_drops_dir_pattern_and_descendants() {
        let gi = matcher_from_lines(&["build/"]);
        assert!(
            ignore_matches(&gi, "build/x", false),
            "child of ignored dir"
        );
        assert!(ignore_matches(&gi, "build", true), "the dir itself");
        assert!(!ignore_matches(&gi, "src/x", false), "tracked code kept");

        // Whitelist beats the ignore.
        let gi = matcher_from_lines(&["node_modules/", "!node_modules/keep.js"]);
        assert!(ignore_matches(&gi, "node_modules/react/index.js", false));
        assert!(!ignore_matches(&gi, "node_modules/keep.js", false));
    }

    #[test]
    fn ignore_cache_reads_gitignore_exclude_and_nested_files() {
        let repo = TempRepo::new("sources");
        repo.write(".gitignore", "build/\n");
        repo.write(".git/info/exclude", "secret-dir/\n");
        // Depth 3 nested file (root = 0) is collected…
        repo.write("crates/inner/.gitignore", "*.tmp\nlog/\n");
        // Depth 4 is beyond the collection limit.
        repo.write("a/b/c/d/.gitignore", "deep/\n");

        let cache = IgnoreCache::build(&repo.0);
        assert!(cache.ignores("build/out.obj", false), "root .gitignore");
        assert!(
            cache.ignores("secret-dir/key.pem", false),
            ".git/info/exclude"
        );
        assert!(
            cache.ignores("crates/inner/a.tmp", false),
            "depth-3 nested .gitignore"
        );
        assert!(
            cache.ignores("crates/inner/log/x", false),
            "nested pattern for its subtree"
        );
        assert!(
            !cache.ignores("a/b/c/d/deep/x", false),
            "depth-4 .gitignore not collected"
        );
        assert!(!cache.ignores("src/main.rs", false), "code kept");
    }

    #[test]
    fn default_ignored_dirs_stay_dropped_without_gitignore() {
        let repo = TempRepo::new("baseline");
        let cache = IgnoreCache::build(&repo.0);
        assert!(
            cache.ignores("node_modules/react/index.js", false),
            "baseline ignore list keeps dependency churn out"
        );
        assert!(cache.ignores("target/debug/x", false));
        assert!(!cache.ignores("src/main.rs", false));
    }

    #[test]
    fn force_rebuild_picks_up_gitignore_edits() {
        let repo = TempRepo::new("rebuild");
        repo.write(".gitignore", "build/\n");
        let mut cache = IgnoreCache::build(&repo.0);
        assert!(!cache.ignores("log-dir/x", false));

        repo.write(".gitignore", "build/\nlog-dir/\n");
        // The event path itself forces the rebuild (mtime granularity on
        // some filesystems is too coarse to catch fast successive writes).
        assert!(is_ignore_source_rel(".gitignore"));
        assert!(is_ignore_source_rel("crates/web/.gitignore"));
        assert!(is_ignore_source_rel(".git/info/exclude"));
        cache.force_rebuild();
        assert!(cache.ignores("log-dir/x", false), "new pattern active");
        assert!(
            cache.ignores("build/out.obj", false),
            "old pattern survives"
        );
    }
}
