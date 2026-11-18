//! Per-repository runtime state: discovery, handles, watcher wiring, and
//! stream bookkeeping (contracts: docs/contracts.md).
//!
//! [`RepoManager`] owns one [`RepoHandle`] per open repo tab. A handle keeps
//! the `git2::Repository` opened once (locked per engine call — git2's
//! `Repository` is `Send` but not `Sync`), the engine instance, the bumpable
//! generation counter the FS watcher advances, per-consumer stream
//! cancellation slots (a new stream for the same repo+kind+consumer cancels
//! that consumer's previous one; other consumers are unaffected), and the
//! full-status snapshot for path-gated incremental status refreshes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tauri::Emitter;

use crate::engine::git_engine::{EngineError, EngineResult, GitEngine, GitEngineM12, GitEngineM3};
use crate::engine::types::{RepoId, RepoInfo, RepoStatus, StatusEntry};
use crate::ops::OpQueue;
use crate::watcher::{self, RepoWatcher};

/// Touched-path limit for incremental status merges: beyond it a full
/// rescan is cheaper than splicing the cached snapshot.
const STATUS_INCREMENTAL_PATH_LIMIT: usize = 200;

/// Construct the engine used for newly opened repositories.
///
/// `Libgit2Engine` (engine/libgit2.rs) is the primary `GitEngine` impl; the
/// factory exists so this module depends on the trait, not the concrete type
/// (capability-table fallbacks land post-M1).
pub fn default_engine() -> Arc<dyn GitEngine> {
    Arc::new(crate::engine::libgit2::Libgit2Engine::new())
}

fn m3_engine() -> Arc<dyn GitEngineM3> {
    Arc::new(crate::engine::libgit2::Libgit2Engine::new())
}

fn m12_engine() -> Arc<dyn GitEngineM12> {
    Arc::new(crate::engine::libgit2::Libgit2Engine::new())
}

/// Cancellation token for one active stream thread.
pub struct StreamHandle {
    cancelled: AtomicBool,
    /// Thread that requested the stream (diagnostics).
    pub owner_thread_id: std::thread::ThreadId,
}

impl StreamHandle {
    fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            owner_thread_id: std::thread::current().id(),
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }
}

/// Which stream surface a token belongs to (per-consumer slot maps are
/// keyed by kind; see [`RepoHandle::begin_stream`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    Log,
    FileHistory,
    Diff,
}

impl StreamKind {
    /// Wire/diagnostic name of the kind.
    pub fn as_str(self) -> &'static str {
        match self {
            StreamKind::Log => "log",
            StreamKind::FileHistory => "file_history",
            StreamKind::Diff => "diff",
        }
    }
}

/// Consumer id used when a stream command does not name one.
const DEFAULT_CONSUMER: &str = "default";

/// Max concurrently installed streams per kind (per repo). Beyond the cap
/// the OLDEST consumer's token is evicted (cancelled) — a hard bound on
/// per-repo stream threads even with buggy clients.
const MAX_STREAMS_PER_KIND: usize = 4;

/// Runtime state for one open repository.
pub struct RepoHandle {
    pub id: RepoId,
    /// Absolute workdir root (no trailing separator, no `\\?\` prefix;
    /// POSIX path on the device for remote repos).
    pub root: PathBuf,
    /// Absolute `.git` directory.
    pub git_dir: PathBuf,
    /// Opened once at discovery; locked for the duration of every engine
    /// call (`git2::Repository` is `Send` but not `Sync`). Only present
    /// for [`RepoBackend::Local`].
    pub repo: Option<Arc<Mutex<git2::Repository>>>,
    /// Where this repo physically lives.
    pub backend: RepoBackend,
    /// Bumped on every watcher batch (or remote poll tick); clients discard
    /// stale stream pages.
    generation: Arc<AtomicU64>,
    engine: Arc<dyn GitEngine>,
    /// M3 power/safety operations (merge, rebase, stash, checkpoints…).
    m3: Arc<dyn GitEngineM3>,
    /// M12: signature verify, branch trash, worktree prune, bisect log.
    m12: Arc<dyn GitEngineM12>,
    /// M2: serial op queue for mutations/net ops (contracts.md M2). M1
    /// read commands bypass it and share the `repo` mutex instead.
    ops: OpQueue,
    /// Per-consumer stream tokens: kind -> (consumer_id, token) pairs in
    /// install order. A new install for the same (kind, consumer) cancels
    /// and replaces only that consumer's token (new view wins within a
    /// consumer); other consumers are unaffected. A Vec, not a map, keeps
    /// insertion order for oldest-eviction at the cap.
    streams: Mutex<StreamSlots>,
    /// Cached full-status snapshot for path-gated incremental refreshes
    /// ([`RepoHandle::status`]); keyed by the generation it was taken at,
    /// so any watcher/mutation bump invalidates it.
    status_cache: Mutex<Option<StatusSnapshot>>,
    closed: AtomicBool,
    watcher: Mutex<Option<RepoWatcher>>,
}

/// A full-status snapshot cached for [`RepoHandle::status`]'s incremental
/// merge, valid while the repo generation is unchanged.
struct StatusSnapshot {
    generation: u64,
    status: RepoStatus,
}

/// Per-kind stream slots: kind name -> (consumer_id, token) pairs in
/// install order (see [`RepoHandle::streams`]).
type StreamSlots = HashMap<&'static str, Vec<(String, Arc<StreamHandle>)>>;

/// Where a repo lives: the local filesystem, or a remote device over SSH.
#[derive(Clone)]
pub enum RepoBackend {
    Local,
    Remote(Arc<RemoteHandle>),
}

/// Per-repo remote state: the device session plus the change poller.
pub struct RemoteHandle {
    pub device_id: String,
    pub device_name: String,
    /// Absolute POSIX workdir path on the device.
    pub root: String,
    pub session: Arc<crate::remote::transport::DeviceSession>,
    /// Repo-change poller (replaces the local FS watcher).
    pub poller: Mutex<Option<crate::remote::polling::RepoPoller>>,
    /// Poll interval seconds (from the device profile).
    pub poll_secs: u32,
}

impl RemoteHandle {
    /// A git engine bound to this repo (cheap: Arc clones).
    pub fn git(&self) -> crate::remote::engine::RemoteGitEngine {
        crate::remote::engine::RemoteGitEngine::new(self.session.clone(), self.root.clone())
    }

    /// An SFTP workspace bound to this repo.
    pub fn workspace(&self) -> crate::remote::workspace::RemoteWorkspace {
        crate::remote::workspace::RemoteWorkspace::new(self.session.clone(), self.root.clone())
    }
}

impl RepoHandle {
    /// Current generation (0 = no watcher activity since open).
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    /// Shared clone of the generation counter (the watcher bumps it).
    pub fn generation_shared(&self) -> Arc<AtomicU64> {
        self.generation.clone()
    }

    pub fn engine(&self) -> Arc<dyn GitEngine> {
        self.engine.clone()
    }

    /// M3 power/safety operations.
    // Unwired until the M3 IPC commands land; nothing reads it yet.
    #[allow(dead_code)]
    pub fn m3(&self) -> Arc<dyn GitEngineM3> {
        self.m3.clone()
    }

    /// M12 operations (signature verify, branch trash, worktree prune,
    /// bisect log).
    pub fn m12(&self) -> Arc<dyn GitEngineM12> {
        self.m12.clone()
    }

    /// M2: this repo's serial op queue (mutations + net ops).
    pub fn ops(&self) -> &OpQueue {
        &self.ops
    }

    /// The local git2 repository (Err for remote repos).
    pub fn repo(&self) -> Result<Arc<Mutex<git2::Repository>>, String> {
        self.repo
            .clone()
            .ok_or_else(|| "this project lives on a remote device".to_string())
    }

    /// The remote backend, when this repo lives on a device.
    pub fn remote(&self) -> Option<&Arc<RemoteHandle>> {
        match &self.backend {
            RepoBackend::Local => None,
            RepoBackend::Remote(remote) => Some(remote),
        }
    }

    pub fn is_remote(&self) -> bool {
        self.remote().is_some()
    }

    /// The device id for remote repos (None for local).
    pub fn device_id(&self) -> Option<String> {
        self.remote().map(|r| r.device_id.clone())
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Current FS-watcher backend (`recommended` | `poll` | `dead`),
    /// mirrored from the debounce thread. `None` for remote repos, which
    /// poll instead of watching (diagnostics: `watch_mode`).
    pub fn watch_mode(&self) -> Option<crate::watcher::WatchMode> {
        self.watcher.lock().as_ref().map(|w| w.mode())
    }

    /// Register a stream token for `(kind, consumer)`, cancelling the
    /// previous token of the SAME consumer only (new view wins within a
    /// consumer; other consumers are unaffected). When the per-kind cap
    /// ([`MAX_STREAMS_PER_KIND`]) would overflow, the oldest consumer's
    /// token is evicted (cancelled). `consumer_id` absent → "default".
    pub fn begin_stream(&self, kind: StreamKind, consumer_id: Option<&str>) -> Arc<StreamHandle> {
        let consumer = consumer_id.unwrap_or(DEFAULT_CONSUMER).to_owned();
        let mut streams = self.streams.lock();
        let slots = streams.entry(kind.as_str()).or_default();
        if let Some(pos) = slots.iter().position(|(id, _)| *id == consumer) {
            let (_, previous) = slots.remove(pos);
            tracing::debug!(
                kind = kind.as_str(),
                consumer = %consumer,
                owner_thread = ?previous.owner_thread_id,
                "begin_stream: cancelled the consumer's previous stream"
            );
            previous.cancel();
        }
        let token = Arc::new(StreamHandle::new());
        slots.push((consumer, token.clone()));
        if slots.len() > MAX_STREAMS_PER_KIND {
            let (_, evicted) = slots.remove(0);
            tracing::debug!(
                kind = kind.as_str(),
                "begin_stream: cap exceeded, evicted the oldest consumer's stream"
            );
            evicted.cancel();
        }
        token
    }

    /// Status with path-gated incremental merge (contracts.md
    /// `repo_status`): with `Some(paths)` on a fresh cache, only those
    /// pathspecs are rescanned and the fresh entries are spliced into the
    /// cached full snapshot; every other case — no paths, stale or absent
    /// cache, git-dir paths (index churn can restage everything), or a
    /// large touched set — falls back to a full rescan, which repopulates
    /// the cache. The returned [`RepoStatus`] has the same shape and
    /// path-sorted entries either way. The cache is keyed by the repo
    /// generation: any watcher batch or mutation bump invalidates it.
    pub fn status(
        &self,
        repo: &git2::Repository,
        paths: Option<&[String]>,
    ) -> EngineResult<RepoStatus> {
        let generation = self.generation();
        // Incremental eligibility (cheap checks first; conservative
        // fallbacks): a non-empty touched set within the size budget, no
        // git-dir paths (index churn can restage everything), and a cache
        // snapshot taken at this exact generation.
        let incremental: Option<(&[String], Vec<StatusEntry>)> = match paths {
            Some(slice)
                if !slice.is_empty()
                    && slice.len() <= STATUS_INCREMENTAL_PATH_LIMIT
                    && !slice.iter().any(|p| p.starts_with(".git/")) =>
            {
                let cache = self.status_cache.lock();
                match cache.as_ref() {
                    Some(snapshot) if snapshot.generation == generation => {
                        Some((slice, snapshot.status.entries.clone()))
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        let status = match incremental {
            // Incremental merge: fresh entries for the touched paths only.
            Some((paths, cached_entries)) => {
                let mut fresh = self.engine.status(repo, Some(paths))?;
                // Same pathspec semantics the engine scan used, so removal
                // and insertion cover exactly the same path set.
                let spec = git2::Pathspec::new(paths.iter().map(|p| p.as_str()))?;
                let mut entries: Vec<StatusEntry> = cached_entries
                    .into_iter()
                    .filter(|entry| {
                        let touched_path =
                            spec.matches_path(Path::new(&entry.path), git2::PathspecFlags::empty());
                        let touched_old = entry.old_path.as_deref().is_some_and(|old| {
                            spec.matches_path(Path::new(old), git2::PathspecFlags::empty())
                        });
                        !(touched_path || touched_old)
                    })
                    .collect();
                entries.append(&mut fresh.entries);
                entries.sort_by(|a, b| {
                    a.path
                        .cmp(&b.path)
                        .then_with(|| a.old_path.cmp(&b.old_path))
                });
                fresh.entries = entries;
                fresh
            }
            // Full rescan.
            None => self.engine.status(repo, None)?,
        };
        // Repopulate the cache only when the repo stayed still during the
        // scan: a mid-scan generation move means the snapshot is already
        // stale and the next call must rescan.
        if self.generation() == generation {
            *self.status_cache.lock() = Some(StatusSnapshot {
                generation,
                status: status.clone(),
            });
        }
        Ok(status)
    }

    /// Tear down: mark closed, cancel streams, stop the watcher (or the
    /// remote poller). Idempotent.
    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for (_, slots) in self.streams.lock().drain() {
            for (_, token) in slots {
                token.cancel();
            }
        }
        // M2 op queue: stop accepting new ops; the worker drains its
        // backlog (jobs still resolve) and then exits.
        self.ops.shutdown();
        // Dropping the watcher joins its debounce thread; dropping the
        // remote poller stops its thread.
        if let Some(remote) = self.remote() {
            drop(remote.poller.lock().take());
        }
        drop(self.watcher.lock().take());
    }
}

impl Drop for RepoHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Owns the handles for every open repo tab.
pub struct RepoManager {
    repos: Mutex<HashMap<RepoId, Arc<RepoHandle>>>,
    id_counter: AtomicU64,
}

impl RepoManager {
    pub fn new() -> Self {
        Self {
            repos: Mutex::new(HashMap::new()),
            id_counter: AtomicU64::new(0),
        }
    }

    pub fn get(&self, id: &RepoId) -> Option<Arc<RepoHandle>> {
        self.repos.lock().get(id).cloned()
    }

    /// Snapshot of every open handle (unordered) — diagnostics
    /// (`diagnostics_bundle`).
    pub fn all(&self) -> Vec<Arc<RepoHandle>> {
        self.repos.lock().values().cloned().collect()
    }

    /// Open (or reuse) the repo at/above `path` and start its FS watcher.
    /// IPC entry point for `repo_open`.
    pub fn open(&self, app: &tauri::AppHandle, path: &Path) -> EngineResult<RepoInfo> {
        let info = self.open_unwatched(path)?;
        if let Some(handle) = self.get(&info.repo_id) {
            // M2: op-progress events flow over the app emitter.
            handle
                .ops()
                .set_sink(Arc::new(crate::ops::TauriEventSink::new(app.clone())));
            let mut slot = handle.watcher.lock();
            // A watcher that exhausted its restart budget AND its poll
            // fallback stays in the slot with a dead thread; replacing it
            // makes "reopen the repo to retry" literally true. Without this
            // check, `repo_open` reused the handle and the repo silently
            // watched nothing forever.
            if slot.as_ref().is_none_or(|w| w.is_dead()) {
                // Revival: the replaced watcher was dead (announced as
                // `alive: false, mode: "dead"`), so announce the fresh one —
                // a state CHANGE, unlike a plain open, which starts
                // alive/recommended and emits nothing.
                let revived = slot.as_ref().is_some_and(|w| w.is_dead());
                let app = app.clone();
                let state_app = app.clone();
                let revival_app = app.clone();
                let repo_id_for_revival = handle.id.clone();
                *slot = Some(RepoWatcher::start(
                    handle.id.clone(),
                    handle.root.clone(),
                    handle.generation_shared(),
                    move |batch| {
                        if let Err(err) = app.emit(watcher::REPO_CHANGED_EVENT, &batch) {
                            tracing::warn!(
                                repo = %batch.repo_id.0,
                                error = %err,
                                "emit repo-changed failed"
                            );
                        }
                    },
                    move |state| {
                        if let Err(err) = state_app.emit(watcher::REPO_WATCH_STATE_EVENT, &state) {
                            tracing::warn!(
                                repo = %state.repo_id.0,
                                mode = ?state.mode,
                                error = %err,
                                "emit repo-watch-state failed"
                            );
                        }
                    },
                ));
                if revived {
                    let revival = watcher::RepoWatchStateEvent {
                        repo_id: repo_id_for_revival,
                        alive: true,
                        mode: watcher::WatchMode::Recommended,
                    };
                    if let Err(err) = revival_app.emit(watcher::REPO_WATCH_STATE_EVENT, &revival) {
                        tracing::warn!(
                            repo = %revival.repo_id.0,
                            error = %err,
                            "emit repo-watch-state revival failed"
                        );
                    }
                }
            }
        }
        Ok(info)
    }

    /// Open a repo without starting a watcher: discovery, bare rejection,
    /// handle insertion. Used by unit tests and by [`RepoManager::open`].
    pub fn open_unwatched(&self, path: &Path) -> EngineResult<RepoInfo> {
        let repo = git2::Repository::discover(path)?;
        if repo.is_bare() {
            return Err(EngineError::Invalid(format!(
                "bare repositories are not supported in M1: {}",
                repo.path().display()
            )));
        }
        let workdir = repo.workdir().ok_or_else(|| {
            EngineError::Invalid(format!(
                "repository has no workdir: {}",
                repo.path().display()
            ))
        })?;
        let root = normalize_root(workdir.to_path_buf());
        let git_dir = normalize_root(repo.path().to_path_buf());

        // Reuse an open handle for the same workdir: one watcher per repo.
        {
            let repos = self.repos.lock();
            for handle in repos.values() {
                if handle.root == root {
                    return Ok(repo_info_of(handle));
                }
            }
        }

        let repo_id = self.next_repo_id();
        let git_dir_for_gc = normalize_root(repo.path().to_path_buf());
        let handle = Arc::new(RepoHandle {
            id: repo_id.clone(),
            ops: OpQueue::new(repo_id),
            root,
            git_dir,
            repo: Some(Arc::new(Mutex::new(repo))),
            backend: RepoBackend::Local,
            generation: Arc::new(AtomicU64::new(0)),
            engine: default_engine(),
            m3: m3_engine(),
            m12: m12_engine(),
            streams: Mutex::new(HashMap::new()),
            status_cache: Mutex::new(None),
            closed: AtomicBool::new(false),
            watcher: Mutex::new(None),
        });
        let info = repo_info_of(&handle);
        self.repos.lock().insert(handle.id.clone(), handle);

        // M12: best-effort checkpoint auto-GC. The marker check + ref walk
        // run on their own thread so even a huge repo never delays an open;
        // the engine reopens its own handle (gc only reads/deletes hidden
        // refs) and every error is swallowed inside `maybe_auto_gc`.
        let _ = std::thread::Builder::new()
            .name("ckpt-auto-gc".to_owned())
            .spawn(move || {
                if let Ok(repo) = git2::Repository::open(&git_dir_for_gc) {
                    crate::engine::checkpoints::maybe_auto_gc(&repo);
                }
            });

        Ok(info)
    }

    /// Close a repo: cancel its streams, stop its watcher, drop the handle.
    /// Returns `false` when the repo was not open. Idempotent.
    pub fn close(&self, id: &RepoId) -> bool {
        match self.repos.lock().remove(id) {
            Some(handle) => {
                // Auth prompts belonging to this repo's net ops resolve as
                // cancelled now, not at their 60 s timeout (registry #4).
                crate::auth::broker().cancel_for_repo(id.0.as_str());
                handle.shutdown();
                true
            }
            None => false,
        }
    }

    /// Open (or reuse) a repo living on a remote device. `session` must be
    /// connected; discovery (toplevel resolve, bare rejection) runs on the
    /// device. Starts the change poller instead of an FS watcher.
    #[allow(clippy::too_many_arguments)]
    pub fn open_remote(
        &self,
        app: &tauri::AppHandle,
        device_id: &str,
        device_name: &str,
        poll_secs: u32,
        session: Arc<crate::remote::transport::DeviceSession>,
        path: &str,
    ) -> Result<RepoInfo, String> {
        let (root, git_dir) = crate::remote::engine::RemoteGitEngine::discover(&session, path)?;

        // Reuse an open handle for the same device+workdir (one poller per
        // repo). Local dedupe keys on root only; remote roots are device-
        // scoped, so compare the device first.
        {
            let repos = self.repos.lock();
            for handle in repos.values() {
                if let Some(remote) = handle.remote() {
                    if remote.device_id == device_id && remote.root == root {
                        let mut info = repo_info_of(handle);
                        info.device_id = Some(device_id.to_string());
                        return Ok(info);
                    }
                }
            }
        }

        let repo_id = self.next_repo_id();
        let remote = Arc::new(RemoteHandle {
            device_id: device_id.to_string(),
            device_name: device_name.to_string(),
            root: root.clone(),
            session,
            poller: Mutex::new(None),
            poll_secs,
        });
        let handle = Arc::new(RepoHandle {
            id: repo_id.clone(),
            ops: OpQueue::new(repo_id.clone()),
            root: PathBuf::from(&root),
            git_dir: PathBuf::from(&git_dir),
            repo: None,
            backend: RepoBackend::Remote(remote.clone()),
            generation: Arc::new(AtomicU64::new(0)),
            engine: default_engine(),
            m3: m3_engine(),
            m12: m12_engine(),
            streams: Mutex::new(HashMap::new()),
            status_cache: Mutex::new(None),
            closed: AtomicBool::new(false),
            watcher: Mutex::new(None),
        });
        let mut info = repo_info_of(&handle);
        info.device_id = Some(device_id.to_string());
        self.repos.lock().insert(handle.id.clone(), handle.clone());

        // Progress events + the change poller (the remote watcher twin).
        handle
            .ops()
            .set_sink(Arc::new(crate::ops::TauriEventSink::new(app.clone())));
        let poller = crate::remote::polling::RepoPoller::start(
            repo_id.clone(),
            handle.generation_shared(),
            Arc::new(remote.git()),
            std::time::Duration::from_secs(poll_secs.max(1) as u64),
            app.clone(),
        )?;
        *remote.poller.lock() = Some(poller);

        Ok(info)
    }

    /// Unique repo id: timestamp + pid + counter, hex — no uuid dependency.
    fn next_repo_id(&self) -> RepoId {
        let counter = self.id_counter.fetch_add(1, Ordering::Relaxed);
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        RepoId(format!("{millis:x}-{:x}-{counter:x}", std::process::id()))
    }
}

impl Default for RepoManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Trim one trailing separator (keeping drive roots like `C:\` intact).
fn normalize_root(path: PathBuf) -> PathBuf {
    let s = path.as_os_str().to_string_lossy();
    if s.len() > 3 && (s.ends_with('/') || s.ends_with('\\')) {
        PathBuf::from(&s[..s.len() - 1])
    } else {
        path
    }
}

fn repo_info_of(handle: &RepoHandle) -> RepoInfo {
    let root = handle.root.to_string_lossy().into_owned();
    let name = handle
        .root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.clone());
    RepoInfo {
        repo_id: handle.id.clone(),
        root,
        name,
        bare: false,
        git_dir: handle.git_dir.to_string_lossy().into_owned(),
        device_id: handle.device_id(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// mygitui project root: `CARGO_MANIFEST_DIR` is `src-tauri`, so one
    /// `pop()` reaches the root that holds `scripts/` and `fixtures/`.
    fn project_root() -> PathBuf {
        let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p
    }

    fn fixtures_basic() -> PathBuf {
        project_root().join("fixtures").join("basic")
    }

    fn fixtures_conflicted() -> PathBuf {
        project_root().join("fixtures").join("conflicted")
    }

    /// Generate fixtures if missing (requires git; `bash` from Git Bash on
    /// Windows). Tolerates a concurrent regeneration by another lane's test
    /// bootstrap: the script wipes and recreates `fixtures/`, so generation
    /// is single-flighted — parallel tests queue on the lock instead of each
    /// running the script and wiping it out from under the others.
    fn ensure_fixtures() {
        static BOOTSTRAP: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
        if fixtures_basic().join(".git").exists() {
            return;
        }
        let _guard = BOOTSTRAP.lock();
        // A sibling test may have finished generating while we waited.
        if fixtures_basic().join(".git").exists() {
            return;
        }
        // On Windows, a bare `bash` on PATH can resolve to the WSL launcher
        // (System32\bash.exe), which exits 1 on distro-less runner images.
        // Fall back to Git for Windows' own bash, which the script targets.
        #[cfg(windows)]
        let candidates: Vec<std::ffi::OsString> = vec![
            "bash".into(),
            "C:\\Program Files\\Git\\bin\\bash.exe".into(),
        ];
        #[cfg(not(windows))]
        let candidates: Vec<std::ffi::OsString> = vec!["bash".into()];
        let mut last_status = None;
        for _ in 0..3 {
            for bash in &candidates {
                let Ok(status) = std::process::Command::new(bash)
                    .arg("scripts/make-fixtures.sh")
                    .current_dir(project_root())
                    .status()
                else {
                    last_status = None;
                    continue;
                };
                last_status = Some(status);
                if status.success() && fixtures_basic().join(".git").exists() {
                    return;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        panic!(
            "scripts/make-fixtures.sh failed (last status: {last_status:?}); fixtures still missing under {}",
            fixtures_basic().display()
        );
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mygitui-repo-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn open_close_roundtrip() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let info = mgr.open_unwatched(&fixtures_basic()).expect("open basic");

        assert!(!info.bare);
        assert_eq!(info.name, "basic");
        assert!(info.root.replace('\\', "/").ends_with("fixtures/basic"));
        assert!(info
            .git_dir
            .replace('\\', "/")
            .ends_with("fixtures/basic/.git"));

        let id = info.repo_id;
        assert!(mgr.get(&id).is_some());
        assert!(mgr.close(&id));
        assert!(mgr.get(&id).is_none());
        assert!(!mgr.close(&id), "close must be idempotent");
    }

    #[test]
    fn discover_from_subdirectory_finds_root() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let from_root = mgr.open_unwatched(&fixtures_basic()).expect("open");
        let from_src = mgr
            .open_unwatched(&fixtures_basic().join("src"))
            .expect("discover from subdir");
        // Same workdir → the open handle is reused.
        assert_eq!(from_root.repo_id, from_src.repo_id);
        assert!(from_src.root.replace('\\', "/").ends_with("fixtures/basic"));
    }

    #[test]
    fn reopen_same_workdir_reuses_handle() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let first = mgr.open_unwatched(&fixtures_basic()).unwrap();
        let second = mgr.open_unwatched(&fixtures_basic()).unwrap();
        assert_eq!(first.repo_id, second.repo_id);
    }

    #[test]
    fn different_repos_get_different_ids() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let basic = mgr.open_unwatched(&fixtures_basic()).unwrap();
        let conflicted = mgr.open_unwatched(&fixtures_conflicted()).unwrap();
        assert_ne!(basic.repo_id, conflicted.repo_id);
        assert!(mgr.close(&basic.repo_id));
        assert!(mgr.close(&conflicted.repo_id));
    }

    #[test]
    fn bare_repo_rejected() {
        let dir = temp_dir("bare");
        git2::Repository::init_opts(&dir, git2::RepositoryInitOptions::new().bare(true))
            .expect("init bare repo");
        let mgr = RepoManager::new();
        let err = mgr.open_unwatched(&dir).expect_err("bare must be rejected");
        assert!(matches!(err, EngineError::Invalid(_)), "got: {err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_repo_rejected() {
        let dir = temp_dir("empty");
        let mgr = RepoManager::new();
        let err = mgr
            .open_unwatched(&dir)
            .expect_err("non-repo path must be rejected");
        assert!(matches!(err, EngineError::Git(_)), "got: {err:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn generation_counter_is_shared() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let info = mgr.open_unwatched(&fixtures_basic()).unwrap();
        let handle = mgr.get(&info.repo_id).unwrap();
        assert_eq!(handle.generation(), 0);
        // The watcher holds exactly this counter.
        handle.generation_shared().fetch_add(1, Ordering::Relaxed);
        assert_eq!(handle.generation(), 1);
        mgr.close(&info.repo_id);
    }

    #[test]
    fn same_consumer_replacement_cancels_and_close_cancels_all() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let info = mgr.open_unwatched(&fixtures_basic()).unwrap();
        let handle = mgr.get(&info.repo_id).unwrap();

        let first = handle.begin_stream(StreamKind::Log, Some("main"));
        assert!(!first.is_cancelled());
        let second = handle.begin_stream(StreamKind::Log, Some("main"));
        assert!(
            first.is_cancelled(),
            "same-consumer replacement cancels the old token"
        );
        assert!(!second.is_cancelled());

        // Other consumers on the same kind are unaffected (new view wins
        // within a consumer only).
        let other = handle.begin_stream(StreamKind::Log, Some("popout"));
        assert!(!second.is_cancelled());
        assert!(!other.is_cancelled());

        // Stream kinds are independent maps.
        let history = handle.begin_stream(StreamKind::FileHistory, None);
        let diff = handle.begin_stream(StreamKind::Diff, None);
        assert!(!second.is_cancelled());
        assert!(!history.is_cancelled());
        assert!(!diff.is_cancelled());

        mgr.close(&info.repo_id);
        assert!(second.is_cancelled());
        assert!(other.is_cancelled());
        assert!(history.is_cancelled());
        assert!(diff.is_cancelled());
        assert!(handle.is_closed());
    }

    #[test]
    fn per_kind_stream_cap_evicts_the_oldest_consumer() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let info = mgr.open_unwatched(&fixtures_basic()).unwrap();
        let handle = mgr.get(&info.repo_id).unwrap();

        let mut tokens = Vec::new();
        for i in 0..MAX_STREAMS_PER_KIND {
            tokens.push(handle.begin_stream(StreamKind::Log, Some(&format!("c{i}"))));
        }
        assert!(
            tokens.iter().all(|t| !t.is_cancelled()),
            "up to the cap nothing is evicted"
        );
        // One more consumer: the oldest entry ("c0") is evicted+cancelled.
        let newest = handle.begin_stream(StreamKind::Log, Some("overflow"));
        assert!(tokens[0].is_cancelled(), "oldest consumer evicted at cap");
        assert!(
            tokens[1..].iter().all(|t| !t.is_cancelled()),
            "later consumers survive eviction"
        );
        assert!(!newest.is_cancelled());

        // Same-consumer re-install does not evict anyone else.
        let replaced = handle.begin_stream(StreamKind::Log, Some("c1"));
        assert!(tokens[1].is_cancelled(), "replaced consumer cancelled");
        assert!(!replaced.is_cancelled());
        assert!(!newest.is_cancelled(), "other consumers untouched");
        mgr.close(&info.repo_id);
    }

    #[test]
    fn incremental_status_merge_matches_full_rescan() {
        use crate::engine::types::ChangeKind;

        let dir = temp_dir("status-incr");
        git2::Repository::init(&dir).expect("init repo");
        let base = dir.join("a.txt");
        let other = dir.join("b.txt");
        std::fs::write(&base, "one\n").unwrap();
        std::fs::write(&other, "two\n").unwrap();
        // The index borrows its repo: keep the handle alive alongside it.
        // Commit both files so the baseline status is clean (unborn HEAD
        // would list the staged files as index-side additions).
        let setup_repo = git2::Repository::open(&dir).unwrap();
        let mut index = setup_repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.add_path(Path::new("b.txt")).unwrap();
        let tree_oid = index.write_tree().unwrap();
        let sig = git2::Signature::now("t", "t@example.test").unwrap();
        {
            let tree = setup_repo.find_tree(tree_oid).unwrap();
            setup_repo
                .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        index.write().unwrap();
        drop(index);
        drop(setup_repo);

        let mgr = RepoManager::new();
        let info = mgr.open_unwatched(&dir).unwrap();
        let handle = mgr.get(&info.repo_id).unwrap();
        let repo = handle.repo().unwrap();

        // Full rescan populates the cache.
        let full = handle.status(&repo.lock(), None).unwrap();
        assert!(full.entries.is_empty(), "clean baseline");

        // Modify a.txt, then an incremental refresh over [a.txt]: the cache
        // is still fresh (no watcher runs here — the touched path is the
        // caller's claim), so the merge path runs and must equal a full
        // rescan (same entries, same order).
        std::fs::write(&base, "one\ntwo\n").unwrap();
        let incremental = handle
            .status(&repo.lock(), Some(&["a.txt".to_string()]))
            .unwrap();
        let full_after = handle.status(&repo.lock(), None).unwrap();
        assert_eq!(incremental.entries, full_after.entries);
        assert_eq!(incremental.entries.len(), 1);
        assert_eq!(incremental.entries[0].path, "a.txt");
        assert_eq!(incremental.entries[0].worktree, ChangeKind::Modified);

        // Delete + add mix in one batch: merge covers both paths.
        std::fs::remove_file(&other).unwrap();
        let added = dir.join("c.txt");
        std::fs::write(&added, "three\n").unwrap();
        let incremental = handle
            .status(
                &repo.lock(),
                Some(&["b.txt".to_string(), "c.txt".to_string()]),
            )
            .unwrap();
        let full_after = handle.status(&repo.lock(), None).unwrap();
        assert_eq!(incremental.entries, full_after.entries);
        assert_eq!(
            incremental
                .entries
                .iter()
                .map(|e| e.path.as_str())
                .collect::<Vec<_>>(),
            vec!["a.txt", "b.txt", "c.txt"],
            "a.txt still modified, b.txt reported deleted, c.txt added, path-sorted"
        );
        assert_eq!(incremental.entries[1].worktree, ChangeKind::Deleted);

        // Staging touches `.git/index`: git-dir paths fall back to a full
        // rescan (conservative — staging can restage everything).
        let staging_repo = git2::Repository::open(&dir).unwrap();
        let mut index = staging_repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        drop(index);
        drop(staging_repo);
        let staged = handle
            .status(&repo.lock(), Some(&[".git/index".to_string()]))
            .unwrap();
        let full_after = handle.status(&repo.lock(), None).unwrap();
        assert_eq!(staged.entries, full_after.entries);
        assert!(
            staged
                .entries
                .iter()
                .any(|e| e.path == "a.txt" && e.index != ChangeKind::Unmodified),
            "staged entry present after the fallback rescan"
        );

        // A generation move invalidates the cache: the merge must not run
        // against a stale snapshot (next call rescans fully and matches).
        handle.generation_shared().fetch_add(1, Ordering::Relaxed);
        std::fs::write(&base, "one\ntwo\nthree\n").unwrap();
        let after_bump = handle
            .status(&repo.lock(), Some(&["a.txt".to_string()]))
            .unwrap();
        let full_after = handle.status(&repo.lock(), None).unwrap();
        assert_eq!(after_bump.entries, full_after.entries);

        mgr.close(&info.repo_id);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
