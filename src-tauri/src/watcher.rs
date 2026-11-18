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
//! # Robustness: restart backoff and the poll fallback
//!
//! A watcher error logs a warning, emits a `full = true` batch so clients
//! resync everything, and restarts the watcher with backoff (3 tries per
//! failure episode, resetting on success). After the budget is spent the
//! thread does NOT give up: it rebuilds the watcher as a
//! `notify::PollWatcher` (directory rescan every ~2s, `compare_contents` on
//! so network mounts / FUSE with unreliable mtimes still report edits) and
//! keeps feeding the exact same event pipeline. While on the poll fallback
//! the thread re-probes the recommended backend every [`RECOMMENDED_RETRY`]
//! (60s) and transparently upgrades back when it arms (state event
//! `"recommended"`). The dead state is reserved for the case where even
//! polling cannot arm (e.g. the workdir vanished): the thread exits, the
//! `dead` flag flips, and `RepoManager::open` replaces the watcher on repo
//! reopen — the revival contract is unchanged, it now costs two failures
//! instead of one.
//!
//! Health transitions (`recommended` → `poll` → `recommended`, or → `dead`)
//! are announced ONCE each on the `repo-watch-state` tauri event (payload:
//! [`RepoWatchStateEvent`]) — never per event. The handle also exposes the
//! current backend via [`RepoWatcher::mode`] for diagnostics.
//!
//! The emitters and the watcher factory are injected as callbacks so the
//! module has no tauri dependency and is unit-testable; `repo` wires them to
//! the `repo-changed` / `repo-watch-state` tauri events. The factory is a
//! test seam ([`WatcherFactory`]): tests can force the recommended backend
//! to fail and drive the poll fallback for real.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::{Event, PollWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use serde::Serialize;
use ts_rs::TS;

use crate::engine::types::RepoId;

/// Debounce window: a batch is emitted after this much FS quiet time.
pub const DEBOUNCE: Duration = Duration::from_millis(200);

/// Force-flush bound: a burst that never stays quiet for [`DEBOUNCE`] (a
/// build writing continuously) ships its batch after at most this long
/// instead of deferring forever.
const MAX_WINDOW: Duration = Duration::from_secs(2);

/// Path cap per batch: beyond this the batch ships early. Clients react to
/// any batch with a resync, so a truncated path list stays correct.
const MAX_PENDING_PATHS: usize = 2000;

/// Tauri event name for emitted batches (registry: docs/contracts.md).
pub const REPO_CHANGED_EVENT: &str = "repo-changed";

/// Tauri event name for watcher health transitions
/// ([`RepoWatchStateEvent`]; registry: docs/contracts.md). Emitted on state
/// CHANGE only — never per event.
pub const REPO_WATCH_STATE_EVENT: &str = "repo-watch-state";

/// Backoff schedule for watcher restarts; after these are exhausted without
/// a successful restart the thread falls back to the poll watcher (see the
/// module docs) instead of dying.
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_millis(200),
    Duration::from_millis(500),
    Duration::from_millis(1000),
];

/// Poll-fallback scan interval: while the recommended backend is unusable
/// the whole workdir tree is rescanned this often.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// While running on the poll fallback, the thread re-probes the recommended
/// backend this often (an upgrade back is transparent when it succeeds).
const RECOMMENDED_RETRY: Duration = Duration::from_secs(60);

/// Idle recv slice while on the poll fallback: bounds how long a quiet tree
/// can delay an upgrade probe past its deadline (the poll scan only wakes
/// the loop on actual changes).
const POLL_IDLE_SLICE: Duration = Duration::from_secs(2);

/// Current watcher backend of one open repo (`RepoWatchStateEvent.mode`,
/// diagnostics `watch_mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    /// notify's recommended backend (inotify / kqueue / FSEvents /
    /// ReadDirectoryChangesW).
    Recommended,
    /// Directory-scan polling fallback (network mounts, FUSE, dead
    /// backends).
    Poll,
    /// The debounce thread exited (even polling failed): the repo must be
    /// reopened to revive watching.
    Dead,
}

/// Payload of the `repo-watch-state` event (registry: docs/contracts.md).
///
/// Emitted on watcher health transitions only: poll fallback engaged
/// (`alive: true, mode: "poll"`), upgrade back to the recommended backend
/// (`alive: true, mode: "recommended"`), death after even the poll fallback
/// failed (`alive: false, mode: "dead"`), and revival on repo reopen
/// (`alive: true, mode: "recommended"`, emitted by `repo`). A fresh open
/// starts alive/recommended and emits nothing.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RepoWatchStateEvent {
    pub repo_id: RepoId,
    pub alive: bool,
    pub mode: WatchMode,
}

/// Payload of the `repo-changed` event (registry: docs/contracts.md).
///
/// JSON shape (exact): `{"repo_id": string, "paths": string[], "head_moved": bool, "full": bool}`.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
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

/// Dependency/build directory names for the fallback flood guard:
/// untracked churn under these (`node_modules` installs, `target` builds,
/// `dist`/`build`/`.next` outputs) never reaches clients. Not a hard drop —
/// the event loop consults the real gitignore matcher first and never
/// drops tracked paths (see `IgnoreCache::drops` / `TrackedIndex`).
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
/// This is the part that needs no repo context — `.git` internals only:
/// - a `.git` component with no successor (the directory itself) → ignored;
/// - the component right after `.git` not in [`ALLOWED_GIT_ENTRIES`] →
///   ignored (deep `.git` internals; lock files such as `index.lock` are
///   dropped because only exact allowed names pass).
///
/// Everything else runs in the event loop: per-repo gitignore matching
/// ([`IgnoreCache`]), the [`IGNORED_DIRS`] fallback for paths the matcher
/// does not cover, and the tracked-path exemption ([`TrackedIndex`]) that
/// keeps versioned files under e.g. `build/` reportable.
pub fn is_ignored_rel(rel: &str) -> bool {
    let comps: Vec<&str> = rel.split('/').filter(|c| !c.is_empty()).collect();
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

/// Whether any path component is an [`IGNORED_DIRS`] dependency/build dir.
/// Fallback flood guard only — the event loop consults it after the
/// gitignore matcher and never for tracked paths.
fn has_ignored_dir_component(rel: &str) -> bool {
    rel.split('/').any(|comp| IGNORED_DIRS.contains(&comp))
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
/// which git expresses by not descending at all. (Lib code goes through
/// [`IgnoreCache::drops`], which also weighs whitelists and the
/// [`IGNORED_DIRS`] fallback.)
#[cfg_attr(not(test), allow(dead_code))]
fn ignore_matches(gi: &Gitignore, rel: &str, is_dir: bool) -> bool {
    matches!(
        gi.matched_path_or_any_parents(rel, is_dir),
        ignore::Match::Ignore(_)
    )
}

/// Mtime-tracked gitignore matcher for one repo root. Built from the root
/// `.gitignore`, nested `.gitignore` files (to [`GITIGNORE_MAX_DEPTH`]),
/// `.git/info/exclude`, and the global `core.excludesFile` — real ignore
/// patterns only. (Dependency/build dirs are NOT baked in here: the
/// [`IGNORED_DIRS`] list is a separate fallback in [`IgnoreCache::drops`]
/// so a tracked file under `build/` can still be reported.) Rebuilt lazily
/// when any source's mtime changed, or forced when a source file is itself
/// an event (it may be brand new).
struct IgnoreCache {
    root: PathBuf,
    gi: Gitignore,
    /// Every file the matcher was built from + its mtime at build time.
    sources: Vec<(PathBuf, Option<SystemTime>)>,
}

impl IgnoreCache {
    fn build(root: &Path) -> IgnoreCache {
        let mut builder = GitignoreBuilder::new(root);
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
    // Test-only since the event loop moved to the full `drops` decision.
    #[cfg_attr(not(test), allow(dead_code))]
    fn ignores(&self, rel: &str, is_dir: bool) -> bool {
        ignore_matches(&self.gi, rel, is_dir)
    }

    /// The full drop decision for one event path (P2 fix): tracked files
    /// always flow — git status reports them no matter what the ignore
    /// rules say, so no flood guard may drop them (a versioned `build/`
    /// script is legal). Otherwise the real gitignore chain decides
    /// (genuinely ignored → drop; an explicit `!` whitelist → flow); only
    /// for paths the chain has NO opinion on does the [`IGNORED_DIRS`]
    /// fallback drop untracked dependency/build churn.
    fn drops(&self, rel: &str, is_dir: bool, tracked: bool) -> bool {
        if tracked {
            return false;
        }
        match self.gi.matched_path_or_any_parents(rel, is_dir) {
            ignore::Match::Ignore(_) => true,
            ignore::Match::Whitelist(_) => false,
            ignore::Match::None => has_ignored_dir_component(rel),
        }
    }
}

/// Sorted snapshot of the git index's file paths, consulted by the
/// [`IGNORED_DIRS`] fallback: a tracked file under `build/` must still
/// produce `repo-changed` events (git status reflects tracked files no
/// matter what the ignore rules say), while `node_modules` churn git
/// itself never tracks stays flood-guarded. Rebuilt when `.git/index`'s
/// mtime changes (checked once per debounce window, like [`IgnoreCache`])
/// or forced when a `.git/index` event arrives (mtime granularity is too
/// coarse for fast successive writes on some filesystems).
struct TrackedIndex {
    /// mtime of the index file at build time.
    seen: Option<SystemTime>,
    /// Sorted repo-relative paths (forward slashes) for O(log n) lookup.
    paths: Vec<String>,
}

impl TrackedIndex {
    fn build(root: &Path) -> TrackedIndex {
        let index_path = git_dir_of(root).map(|git_dir| git_dir.join("index"));
        let seen = index_path.as_deref().and_then(mtime_of);
        let mut paths = Vec::new();
        // Best-effort like the ignore sources: a repo that will not open
        // (bare, locked, mid-clone) contributes no tracked paths, and the
        // fallback stays fully conservative (drops everything under
        // IGNORED_DIRS it does not know to be tracked).
        if let Ok(repo) = git2::Repository::open(root) {
            if let Ok(index) = repo.index() {
                for entry in index.iter() {
                    paths.push(String::from_utf8_lossy(&entry.path).into_owned());
                }
            }
        }
        paths.sort();
        paths.dedup();
        TrackedIndex { seen, paths }
    }

    /// Rebuild when the index changed on disk (stage, commit, checkout,
    /// checkpoint restore...).
    fn refresh_if_changed(&mut self, root: &Path) {
        let index_path = git_dir_of(root).map(|git_dir| git_dir.join("index"));
        if index_path.as_deref().and_then(mtime_of) != self.seen {
            *self = TrackedIndex::build(root);
        }
    }

    /// Whether `rel` (a repo-relative file path) is in the git index.
    fn contains(&self, rel: &str) -> bool {
        self.paths
            .binary_search_by(|probe| probe.as_str().cmp(rel))
            .is_ok()
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

/// Watcher construction seam: builds the notify backend for `mode` bound to
/// `tx`'s feed channel and installs the initial recursive watch on `root`.
/// Production uses [`real_factory`]; tests inject a factory that can force
/// backend failures to drive the restart / poll-fallback paths for real.
type WatcherFactory =
    dyn Fn(&Path, &Sender<Feed>, WatchMode) -> Option<Box<dyn Watcher + Send>> + Send;

/// Handle to a running watcher; dropping it shuts the watcher down (the
/// debounce thread is joined in [`Drop`]).
pub struct RepoWatcher {
    tx: Sender<Feed>,
    thread: Option<JoinHandle<()>>,
    /// Flipped when the debounce thread exits for any reason. A
    /// slot-resident watcher can only have exited by exhausting its restart
    /// budget AND the poll fallback (`Feed::Shutdown` is sent solely by
    /// [`Drop`], which removes the watcher from consideration) —
    /// `RepoManager::open` uses this to replace a dead watcher instead of
    /// reusing it forever.
    dead: Arc<AtomicBool>,
    /// Current backend, mirrored from the debounce thread (diagnostics).
    mode: Arc<Mutex<WatchMode>>,
}

impl RepoWatcher {
    /// Watch `root` recursively and call `on_batch` for every debounced,
    /// filtered batch; call `on_state` once per watcher health transition
    /// (`repo-watch-state`, change-only). `generation` is bumped (by one) on
    /// every emitted batch so stream consumers can detect staleness.
    pub fn start(
        repo_id: RepoId,
        root: PathBuf,
        generation: Arc<AtomicU64>,
        on_batch: impl Fn(RepoChangedEvent) + Send + 'static,
        on_state: impl Fn(RepoWatchStateEvent) + Send + 'static,
    ) -> RepoWatcher {
        RepoWatcher::spawn(
            repo_id,
            root,
            generation,
            Box::new(on_batch),
            Box::new(on_state),
            Box::new(real_factory),
            RECOMMENDED_RETRY,
        )
    }

    /// Test seam: everything [`RepoWatcher::start`] does, but with the
    /// watcher factory and the recommended-backend retry cadence injected so
    /// a test can force the recommended backend to fail, exercise the poll
    /// fallback, and time the upgrade probe.
    #[cfg(test)]
    fn start_seamed(
        repo_id: RepoId,
        root: PathBuf,
        generation: Arc<AtomicU64>,
        on_batch: Box<dyn Fn(RepoChangedEvent) + Send>,
        on_state: Box<dyn Fn(RepoWatchStateEvent) + Send>,
        factory: Box<WatcherFactory>,
        upgrade_after: Duration,
    ) -> RepoWatcher {
        RepoWatcher::spawn(
            repo_id,
            root,
            generation,
            on_batch,
            on_state,
            factory,
            upgrade_after,
        )
    }

    fn spawn(
        repo_id: RepoId,
        root: PathBuf,
        generation: Arc<AtomicU64>,
        on_batch: Box<dyn Fn(RepoChangedEvent) + Send>,
        on_state: Box<dyn Fn(RepoWatchStateEvent) + Send>,
        factory: Box<WatcherFactory>,
        upgrade_after: Duration,
    ) -> RepoWatcher {
        let (tx, rx) = mpsc::channel::<Feed>();
        let dead = Arc::new(AtomicBool::new(false));
        let mode = Arc::new(Mutex::new(WatchMode::Recommended));
        let thread = thread::Builder::new()
            .name(format!("watcher-{}", repo_id.0))
            .spawn({
                let tx = tx.clone();
                let dead = dead.clone();
                let mode = mode.clone();
                move || {
                    let ctx = BatchCtx {
                        repo_id,
                        root,
                        generation,
                        on_batch,
                        on_state,
                        factory,
                        upgrade_after,
                        mode_slot: mode,
                    };
                    run(rx, tx, ctx);
                    dead.store(true, Ordering::Relaxed);
                }
            })
            .expect("watcher thread spawn");
        RepoWatcher {
            tx,
            thread: Some(thread),
            dead,
            mode,
        }
    }

    /// True once the debounce thread has exited (restart budget spent AND
    /// the poll fallback failed to arm).
    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// Current watcher backend, as last announced by the debounce thread
    /// (diagnostics: `watch_mode`). `Dead` only after the thread exited.
    pub fn mode(&self) -> WatchMode {
        *self.mode.lock()
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

/// Everything the debounce thread needs to emit a batch or a state change.
struct BatchCtx {
    repo_id: RepoId,
    root: PathBuf,
    generation: Arc<AtomicU64>,
    on_batch: Box<dyn Fn(RepoChangedEvent) + Send>,
    /// Watcher health transitions (`repo-watch-state`, change-only).
    on_state: Box<dyn Fn(RepoWatchStateEvent) + Send>,
    /// Watcher construction seam (production: [`real_factory`]).
    factory: Box<WatcherFactory>,
    /// How often the poll fallback re-probes the recommended backend
    /// (production: [`RECOMMENDED_RETRY`]; test-injected for fast upgrades).
    upgrade_after: Duration,
    /// The shared slot mirroring the current backend into the handle
    /// ([`RepoWatcher::mode`], diagnostics).
    mode_slot: Arc<Mutex<WatchMode>>,
}

impl BatchCtx {
    /// Announce one watcher health transition: mirror it into the handle's
    /// mode slot (diagnostics) and fire the state callback exactly once.
    fn emit_state(&self, mode: WatchMode) {
        *self.mode_slot.lock() = mode;
        (self.on_state)(RepoWatchStateEvent {
            repo_id: self.repo_id.clone(),
            alive: mode != WatchMode::Dead,
            mode,
        });
    }
}

/// Poll-fallback notify config (unit-tested builder): a modest ~2s
/// directory rescan; `compare_contents` hashes changed files so network
/// mounts / FUSE whose mtimes lie still report real edits.
fn poll_watcher_config() -> notify::Config {
    notify::Config::default()
        .with_poll_interval(POLL_INTERVAL)
        .with_compare_contents(true)
}

/// Production [`WatcherFactory`]: creates the notify backend for `mode`
/// (recommended, or the poll fallback) and installs the initial recursive
/// watch. Returns `None` on creation or initial-watch failure (already
/// warned).
fn real_factory(
    root: &Path,
    tx: &Sender<Feed>,
    mode: WatchMode,
) -> Option<Box<dyn Watcher + Send>> {
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
    let mut watcher: Box<dyn Watcher + Send> = match mode {
        WatchMode::Recommended => Box::new(notify::recommended_watcher(callback).ok()?),
        WatchMode::Poll => Box::new(PollWatcher::new(callback, poll_watcher_config()).ok()?),
        WatchMode::Dead => return None,
    };
    match watcher.watch(root, RecursiveMode::Recursive) {
        Ok(()) => Some(watcher),
        Err(err) => {
            tracing::warn!(root = %root.display(), error = %err, "watcher: initial watch failed");
            None
        }
    }
}

/// Create the notify watcher for `root` via the injected factory. Returns
/// `None` on failure (already warned). The poll backend additionally
/// refuses to arm on a root that is not an existing directory: a poller on
/// a vanished root would only ever produce scan errors, and the thread must
/// go dead (revival contract: reopen the repo) instead of spinning forever.
fn create_watcher(
    root: &Path,
    tx: &Sender<Feed>,
    mode: WatchMode,
    factory: &WatcherFactory,
) -> Option<Box<dyn Watcher + Send>> {
    if mode == WatchMode::Dead {
        return None;
    }
    if mode == WatchMode::Poll && !root.is_dir() {
        tracing::warn!(root = %root.display(), "watcher: poll fallback refused (root gone)");
        return None;
    }
    factory(root, tx, mode)
}

/// Try to (re)create the recommended watcher, sleeping through the backoff
/// schedule. `attempts` counts consecutive failures; a success resets it.
/// Returns `None` when the budget is spent — a final `full = true` event is
/// emitted so clients resync, and the caller engages the poll fallback.
fn restart(
    ctx: &BatchCtx,
    tx: &Sender<Feed>,
    attempts: &mut usize,
) -> Option<Box<dyn Watcher + Send>> {
    while *attempts < RETRY_BACKOFF.len() {
        let wait = RETRY_BACKOFF[*attempts];
        *attempts += 1;
        thread::sleep(wait);
        tracing::warn!(
            root = %ctx.root.display(),
            attempt = *attempts,
            "watcher: restarting with backoff"
        );
        if let Some(watcher) = create_watcher(&ctx.root, tx, WatchMode::Recommended, &*ctx.factory)
        {
            *attempts = 0;
            return Some(watcher);
        }
    }
    tracing::error!(
        root = %ctx.root.display(),
        tries = RETRY_BACKOFF.len(),
        "watcher: recommended backend dead after exhausted restarts; falling back to polling"
    );
    emit(ctx, Vec::new(), false, true);
    None
}

/// Engage the poll fallback after the recommended backend's restart budget
/// is spent: rebuild the watcher as a PollWatcher and announce the
/// transition (`alive: true, mode: "poll"`). Returns `None` (emitting
/// nothing — the caller owns the dead announcement) when even the poller
/// cannot arm.
fn enter_poll_mode(ctx: &BatchCtx, tx: &Sender<Feed>) -> Option<Box<dyn Watcher + Send>> {
    tracing::warn!(
        root = %ctx.root.display(),
        poll_interval_secs = POLL_INTERVAL.as_secs(),
        "watcher: falling back to directory polling"
    );
    let watcher = create_watcher(&ctx.root, tx, WatchMode::Poll, &*ctx.factory);
    if watcher.is_some() {
        ctx.emit_state(WatchMode::Poll);
    }
    watcher
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
fn run(rx: Receiver<Feed>, tx: Sender<Feed>, ctx: BatchCtx) {
    // M12: gitignore-aware filtering. Built once per watcher lifetime and
    // refreshed lazily (source mtimes) or forced (a .gitignore event).
    let mut ignores = IgnoreCache::build(&ctx.root);
    // P2: the git index's tracked paths, for the IGNORED_DIRS fallback —
    // a tracked file under build/ must still produce events.
    let mut tracked = TrackedIndex::build(&ctx.root);

    let mut attempts = 0usize;
    let mut mode = WatchMode::Recommended;
    // While on the poll fallback: when to re-probe the recommended backend
    // (upgrade back is transparent; the probe runs once per window).
    let mut upgrade_at = Instant::now();
    let mut watcher = create_watcher(&ctx.root, &tx, mode, &*ctx.factory);
    if watcher.is_none() {
        watcher = restart(&ctx, &tx, &mut attempts);
        if watcher.is_none() {
            // Recommended budget spent before the loop even started (e.g.
            // the root never existed): fall back to polling before giving
            // up — same contract as the errored path below.
            watcher = enter_poll_mode(&ctx, &tx);
            if watcher.is_none() {
                // Even the poller could not arm: without a watcher there
                // can be no events, so park-looping on `rx` would wedge the
                // thread alive-but-inert forever. Announce the death, exit,
                // and let the `dead` flag flip so `RepoManager::open`
                // replaces us on the next reopen.
                ctx.emit_state(WatchMode::Dead);
                return;
            }
            mode = WatchMode::Poll;
            upgrade_at = Instant::now() + ctx.upgrade_after;
        }
    }

    'events: loop {
        // Poll fallback: periodically attempt to upgrade back to the
        // recommended backend (a quiet tree only wakes the loop via the
        // idle slice below, so the probe also runs there).
        if mode == WatchMode::Poll && Instant::now() >= upgrade_at {
            match create_watcher(&ctx.root, &tx, WatchMode::Recommended, &*ctx.factory) {
                Some(w) => {
                    tracing::info!(
                        root = %ctx.root.display(),
                        "watcher: recommended backend recovered; leaving poll fallback"
                    );
                    watcher = Some(w); // drops the poll watcher
                    mode = WatchMode::Recommended;
                    attempts = 0;
                    ctx.emit_state(WatchMode::Recommended);
                }
                None => {
                    upgrade_at = Instant::now() + ctx.upgrade_after;
                }
            }
        }

        // Re-check the ignore sources and the git index once per debounce
        // window.
        ignores.refresh_if_changed();
        tracked.refresh_if_changed(&ctx.root);

        let mut pending: Vec<String> = Vec::new();
        let mut head_moved = false;
        let mut errored = false;
        // Force-flush bounds: the quiet-window debounce alone lets a burst
        // that never stays quiet (a build writing every <DEBOUNCE) defer
        // its batch forever while `pending` grows. Ship the batch after
        // MAX_WINDOW regardless, or earlier when the path list is full.
        let window_deadline = Instant::now() + MAX_WINDOW;

        // Collect one debounce window: the first feed blocks, then we keep
        // folding events until DEBOUNCE passes with no traffic (or a
        // force-flush bound hits). On the poll fallback the first feed
        // waits at most POLL_IDLE_SLICE instead: a quiet tree would
        // otherwise block forever, starving the upgrade probe above.
        loop {
            let feed = if pending.is_empty() && !errored {
                if mode == WatchMode::Poll {
                    match rx.recv_timeout(POLL_IDLE_SLICE) {
                        Ok(feed) => feed,
                        Err(mpsc::RecvTimeoutError::Timeout) => break, // quiet: probe check at loop top
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                } else {
                    match rx.recv() {
                        Ok(feed) => feed,
                        Err(_) => return, // every sender dropped: paranoia path
                    }
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
                        if rel == ".git/index" {
                            // Staging changed the tracked set: rebuild
                            // before filtering the rest of the window
                            // (mtime granularity is too coarse to catch
                            // fast successive index writes).
                            tracked = TrackedIndex::build(&ctx.root);
                        }
                        let is_dir = std::fs::metadata(strip_verbatim(path))
                            .map(|m| m.is_dir())
                            .unwrap_or(false);
                        if ignores.drops(&rel, is_dir, tracked.contains(&rel)) {
                            continue;
                        }
                        if is_head_move_rel(&rel) {
                            head_moved = true;
                        }
                        pending.push(rel);
                    }
                    if pending.len() >= MAX_PENDING_PATHS || Instant::now() >= window_deadline {
                        break;
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
                // Recommended budget spent: keep watching via polling
                // instead of dying. Revival (`RepoManager::open` replacing
                // a dead watcher) only applies when even this fails.
                watcher = enter_poll_mode(&ctx, &tx);
                if watcher.is_none() {
                    // Even the poll fallback could not arm: dead. Clients
                    // already resynced (the `full = true` batch above); the
                    // state event lets the UI flag the repo as unwatched.
                    tracing::error!(
                        root = %ctx.root.display(),
                        "watcher: dead after exhausted restarts and poll fallback; reopen the repo to retry"
                    );
                    ctx.emit_state(WatchMode::Dead);
                    break 'events;
                }
                mode = WatchMode::Poll;
                upgrade_at = Instant::now() + ctx.upgrade_after;
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
        // IGNORED_DIRS is no longer a hard drop here: tracked files under
        // build/ must flow — the fallback lives in `IgnoreCache::drops`.
        assert!(!is_ignored_rel("node_modules/react/index.js"));
        assert!(!is_ignored_rel("crates/target/debug/x"));
        assert!(!is_ignored_rel("web/.next/static/x.js"));
        assert!(!is_ignored_rel("dist/bundle.js"));
        assert!(!is_ignored_rel("build/out.obj"));
        assert!(!is_ignored_rel("apps/web/node_modules/x"));
    }

    #[test]
    fn ignored_dir_component_fallback_scope() {
        // Dependency/build directories at any depth.
        assert!(has_ignored_dir_component("node_modules/react/index.js"));
        assert!(has_ignored_dir_component("crates/target/debug/x"));
        assert!(has_ignored_dir_component("web/.next/static/x.js"));
        assert!(has_ignored_dir_component("dist/bundle.js"));
        assert!(has_ignored_dir_component("build/out.obj"));
        assert!(has_ignored_dir_component("apps/web/node_modules/x"));
        // Whole-component match only: a name merely starting with one of
        // the entries is a normal path.
        assert!(!has_ignored_dir_component("src/main.py"));
        assert!(!has_ignored_dir_component("buildsystem/config.rs"));
        assert!(!has_ignored_dir_component("distutils/setup.py"));
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
            |_| {},
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
        // Without any ignore file the gitignore chain has no opinion, so
        // the IGNORED_DIRS fallback decides (untracked churn stays out).
        assert!(
            cache.drops("node_modules/react/index.js", false, false),
            "baseline ignore list keeps dependency churn out"
        );
        assert!(cache.drops("target/debug/x", false, false));
        assert!(!cache.drops("src/main.rs", false, false));
        // ...but never drops tracked files (P2: a versioned build/ script
        // must still produce repo-changed events).
        assert!(!cache.drops("build/out.obj", false, true));
        assert!(!cache.drops("node_modules/react/index.js", false, true));
    }

    /// The P2 fix end to end: a tracked file under `build/` produces an
    /// event; a gitignored file under `node_modules/` still does not.
    #[test]
    fn tracked_file_under_build_flows_gitignored_node_modules_does_not() {
        let repo = TempRepo::new("tracked-build");
        repo.write(".gitignore", "node_modules/\n");
        repo.write("build/out.txt", "generated");
        repo.write("node_modules/pkg/index.js", "vendored");
        {
            let r = git2::Repository::open(&repo.0).expect("open temp repo");
            let mut index = r.index().expect("index");
            index.add_path(Path::new("build/out.txt")).expect("add");
            index.write().expect("write index");
        }
        let cache = IgnoreCache::build(&repo.0);
        let tracked = TrackedIndex::build(&repo.0);
        assert!(
            tracked.contains("build/out.txt"),
            "the staged path is in the index snapshot"
        );
        assert!(
            !cache.drops("build/out.txt", false, tracked.contains("build/out.txt")),
            "tracked file under build/ must produce an event"
        );
        assert!(
            cache.drops("build/out.txt", false, false),
            "the same path untracked stays flood-guarded"
        );
        assert!(
            cache.drops(
                "node_modules/pkg/index.js",
                false,
                tracked.contains("node_modules/pkg/index.js")
            ),
            "gitignored node_modules churn must not produce events"
        );
        assert!(!cache.drops("src/main.rs", false, false), "code kept");
    }

    #[test]
    fn tracked_index_rebuilds_when_the_index_changes() {
        let repo = TempRepo::new("tracked-refresh");
        repo.write("a/build/thing.txt", "x");
        let mut tracked = TrackedIndex::build(&repo.0);
        assert!(
            !tracked.contains("a/build/thing.txt"),
            "unstaged path unknown to the snapshot"
        );
        {
            let r = git2::Repository::open(&repo.0).expect("open temp repo");
            let mut index = r.index().expect("index");
            index.add_path(Path::new("a/build/thing.txt")).expect("add");
            index.write().expect("write index");
        }
        // Same mtime on fast filesystems — force the rebuild like the
        // `.git/index` event path does.
        tracked = TrackedIndex::build(&repo.0);
        assert!(tracked.contains("a/build/thing.txt"));
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

    /// A watcher whose every (re)start attempt fails — including the poll
    /// fallback (the root never exists, so the poller refuses to arm on a
    /// vanished root) — exhausts its budget, exits the debounce thread and
    /// must flag itself dead — `RepoManager::open` keys the revival on this
    /// ("reopen the repo to retry"). The dead transition is announced once
    /// on the state callback (`alive: false, mode: "dead"`).
    #[test]
    fn exhausted_restarts_flip_the_dead_flag() {
        let dead_root = std::env::temp_dir().join(format!(
            "mygitui-watcher-dead-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Deliberately never created: every backend fails.
        let watcher = RepoWatcher::start(
            RepoId("watcher-dead-test".to_owned()),
            dead_root,
            Arc::new(AtomicU64::new(0)),
            |_| {},
            |_| {},
        );
        assert!(!watcher.is_dead(), "fresh watcher is alive");
        assert_eq!(watcher.mode(), WatchMode::Recommended);
        // 3 backoff attempts (200+500+1000 ms) plus the poll-fallback probe
        // and scheduler slack.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !watcher.is_dead() {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            watcher.is_dead(),
            "watcher must flag itself dead after exhausting restarts"
        );
        assert_eq!(
            watcher.mode(),
            WatchMode::Dead,
            "the mode slot mirrors the death"
        );
    }

    /// The watch-state payload serializes exactly as contracted (snake_case
    /// mode values, `alive` bool).
    #[test]
    fn watch_state_event_json_shape() {
        let event = RepoWatchStateEvent {
            repo_id: RepoId("r".to_owned()),
            alive: false,
            mode: WatchMode::Dead,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["repo_id"], "r");
        assert_eq!(json["alive"], false);
        assert_eq!(json["mode"], "dead");
        let event = RepoWatchStateEvent {
            repo_id: RepoId("r".to_owned()),
            alive: true,
            mode: WatchMode::Poll,
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["mode"], "poll");
        assert_eq!(json["alive"], true);
    }

    /// The poll-watcher config builder: ~2s scan interval, content
    /// comparison on (network mounts / FUSE can report bogus mtimes).
    #[test]
    fn poll_watcher_config_scans_every_2s_with_content_compare() {
        let config = poll_watcher_config();
        assert_eq!(config.poll_interval(), Some(POLL_INTERVAL));
        assert_eq!(config.poll_interval(), Some(Duration::from_secs(2)));
        assert!(config.compare_contents(), "network mounts need hashing");
    }

    /// Item 61 end to end: with the recommended backend forced to fail, the
    /// watcher arms a PollWatcher instead, keeps feeding the same event
    /// pipeline (a file write arrives through the poll scan), announces the
    /// `poll` state, and — once the recommended backend recovers —
    /// transparently upgrades back and announces `recommended`.
    #[test]
    fn poll_fallback_watches_and_upgrades_back_to_recommended() {
        struct TempRepo(PathBuf);
        impl TempRepo {
            fn new(tag: &str) -> TempRepo {
                let dir = std::env::temp_dir()
                    .join(format!("mygitui-wpoll-{}-{tag}", std::process::id()));
                let _ = std::fs::remove_dir_all(&dir);
                std::fs::create_dir_all(&dir).expect("create temp dir");
                git2::Repository::init(&dir).expect("init temp repo");
                TempRepo(dir)
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
        let repo = TempRepo::new("fallback");

        // The seam: the recommended backend fails until `allow` flips; the
        // poll backend is a real PollWatcher with a fast scan for test speed.
        let allow = Arc::new(AtomicBool::new(false));
        let factory_allow = allow.clone();
        let factory = Box::new(move |root: &Path, tx: &Sender<Feed>, mode: WatchMode| {
            match mode {
                WatchMode::Recommended => {
                    if !factory_allow.load(Ordering::SeqCst) {
                        return None;
                    }
                    real_factory(root, tx, mode)
                }
                // The fallback: same callback wiring, 150ms scan so the
                // test does not wait out the production 2s cadence.
                WatchMode::Poll => {
                    let tx = tx.clone();
                    let mut poll = PollWatcher::new(
                        move |res: Result<Event, notify::Error>| {
                            let feed = match res {
                                Ok(event) => Feed::Event(event),
                                Err(err) => Feed::Error(err),
                            };
                            let _ = tx.send(feed);
                        },
                        notify::Config::default()
                            .with_poll_interval(Duration::from_millis(150))
                            .with_compare_contents(true),
                    )
                    .expect("poll watcher");
                    poll.watch(root, RecursiveMode::Recursive)
                        .expect("poll watch");
                    Some(Box::new(poll) as Box<dyn Watcher + Send>)
                }
                WatchMode::Dead => None,
            }
        });

        let (batch_tx, batch_rx) = mpsc::channel();
        let (state_tx, state_rx) = mpsc::channel();
        let watcher = RepoWatcher::start_seamed(
            RepoId("watcher-fallback-test".to_owned()),
            repo.0.clone(),
            Arc::new(AtomicU64::new(0)),
            Box::new(move |batch| {
                let _ = batch_tx.send(batch);
            }),
            Box::new(move |state| {
                let _ = state_tx.send(state);
            }),
            factory,
            Duration::from_millis(400), // upgrade probe cadence (test-timed)
        );

        // Fresh watcher starts on the (forced-failing) recommended backend.
        assert!(!watcher.is_dead());
        // The poll fallback engages: state event announces it, the mode
        // slot mirrors it.
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && watcher.mode() != WatchMode::Poll {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            watcher.mode(),
            WatchMode::Poll,
            "poll fallback must engage when the recommended backend always fails"
        );
        let poll_state = state_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("poll state event");
        assert!(poll_state.alive, "poll fallback is a live watcher");
        assert_eq!(poll_state.mode, WatchMode::Poll);
        assert_eq!(poll_state.repo_id.0, "watcher-fallback-test");

        // The same event pipeline feeds through the poller: a new file
        // shows up in a batch (poll scans every 150ms).
        std::fs::write(repo.0.join("poll-watched.txt"), b"via polling\n").expect("write file");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while Instant::now() < deadline {
            match batch_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(batch) => {
                    if batch.paths.iter().any(|p| p == "poll-watched.txt") {
                        found = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(
            found,
            "the poll fallback must feed the repo-changed pipeline"
        );

        // Recover the recommended backend: the upgrade probe (every 400ms
        // here) arms it and the state event announces the upgrade.
        allow.store(true, Ordering::SeqCst);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && watcher.mode() != WatchMode::Recommended {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            watcher.mode(),
            WatchMode::Recommended,
            "the loop must upgrade back to the recommended backend"
        );
        // The upgrade announcement: skip any interleaved re-announcement
        // (a rare mid-test poll error re-engages `poll` first) until the
        // `recommended` upgrade arrives.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut upgrade_state = None;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match state_rx.recv_timeout(remaining) {
                Ok(state) if state.mode == WatchMode::Recommended => {
                    upgrade_state = Some(state);
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let upgrade_state = upgrade_state.expect("upgrade state event");
        assert!(upgrade_state.alive, "the upgrade is a live watcher");

        // Still watching: another write arrives (now event-driven).
        std::fs::write(repo.0.join("post-upgrade.txt"), b"via inotify\n").expect("write file");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut found = false;
        while Instant::now() < deadline {
            match batch_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(batch) => {
                    if batch.paths.iter().any(|p| p == "post-upgrade.txt") {
                        found = true;
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        assert!(found, "watching must continue after the upgrade");
        drop(watcher);
    }
}
