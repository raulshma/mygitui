//! Recursive filesystem watcher for one open repository's workdir.
//!
//! Wraps `notify::recommended_watcher` behind a debounce thread: events are
//! collected and collapsed until the filesystem has been quiet for
//! [`DEBOUNCE`] (200 ms), then one batch is emitted. Batches are pre-filtered
//! so clients never see noise: git object-store churn is dropped, only the
//! `.git` metadata entries that can change HEAD/status pass through, and
//! dependency/build directories are skipped (full ignore parsing arrives with
//! engine integration).
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
use std::time::Duration;

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

/// M1 ignore filter on a repo-relative, forward-slash path.
///
/// - any component in [`IGNORED_DIRS`] → ignored;
/// - a `.git` component with no successor (the directory itself) → ignored;
/// - the component right after `.git` not in [`ALLOWED_GIT_ENTRIES`] →
///   ignored (deep `.git` internals; lock files such as `index.lock` are
///   dropped because only exact allowed names pass).
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

    let mut attempts = 0usize;
    let mut watcher = create_watcher(&ctx.root, &tx);
    if watcher.is_none() {
        watcher = restart(&ctx, &tx, &mut attempts);
    }

    'events: loop {
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
        let root = Path::new("C:/Code/mygitui/fixtures/basic");
        let path = Path::new("C:/Code/mygitui/fixtures/basic/src/main.py");
        assert_eq!(relativize(root, path), "src/main.py");

        let root_bs = Path::new(r"C:\Code\mygitui\fixtures\basic\");
        let path_bs = Path::new(r"C:\Code\mygitui\fixtures\basic\src\main.py");
        assert_eq!(relativize(root_bs, path_bs), "src/main.py");
    }

    #[test]
    fn relativize_strips_verbatim_prefix() {
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
}
