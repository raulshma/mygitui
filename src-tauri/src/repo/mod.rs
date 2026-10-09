//! Per-repository runtime state: discovery, handles, watcher wiring, and
//! stream bookkeeping (contracts: docs/contracts.md).
//!
//! [`RepoManager`] owns one [`RepoHandle`] per open repo tab. A handle keeps
//! the `git2::Repository` opened once (locked per engine call — git2's
//! `Repository` is `Send` but not `Sync`), the engine instance, the bumpable
//! generation counter the FS watcher advances, the graph [`LaneState`] used
//! for log-stream continuation, and cancellation slots for the active
//! log/file-history streams (a new stream for the same repo+kind cancels the
//! previous one).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tauri::Emitter;

use crate::engine::git_engine::{EngineError, EngineResult, GitEngine, GitEngineM12, GitEngineM3};
use crate::engine::types::{RepoId, RepoInfo};
use crate::graph::types::LaneState;
use crate::ops::OpQueue;
use crate::watcher::{self, RepoWatcher};

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

/// Install a fresh stream token in `slot`, cancelling the previous tenant.
fn install_stream(slot: &Mutex<Option<Arc<StreamHandle>>>) -> Arc<StreamHandle> {
    let mut guard = slot.lock();
    if let Some(previous) = guard.take() {
        tracing::debug!(
            owner_thread = ?previous.owner_thread_id,
            "install_stream: cancelled previous stream"
        );
        previous.cancel();
    }
    let token = Arc::new(StreamHandle::new());
    *guard = Some(token.clone());
    token
}

/// Runtime state for one open repository.
pub struct RepoHandle {
    pub id: RepoId,
    /// Absolute workdir root (no trailing separator, no `\\?\` prefix).
    pub root: PathBuf,
    /// Absolute `.git` directory.
    pub git_dir: PathBuf,
    /// Opened once at discovery; locked for the duration of every engine
    /// call (`git2::Repository` is `Send` but not `Sync`).
    pub repo: Arc<Mutex<git2::Repository>>,
    /// Bumped on every watcher batch; clients discard stale stream pages.
    generation: Arc<AtomicU64>,
    engine: Arc<dyn GitEngine>,
    /// M3 power/safety operations (merge, rebase, stash, checkpoints…).
    m3: Arc<dyn GitEngineM3>,
    /// M12: signature verify, branch trash, worktree prune, bisect log.
    m12: Arc<dyn GitEngineM12>,
    /// M2: serial op queue for mutations/net ops (contracts.md M2). M1
    /// read commands bypass it and share the `repo` mutex instead.
    ops: OpQueue,
    /// Graph lane layout state carried between log-stream pages.
    pub lanes: Mutex<LaneState>,
    /// Active log stream; a new request cancels the previous.
    log_stream: Mutex<Option<Arc<StreamHandle>>>,
    /// Active file-history stream; independent of `log_stream`.
    history_stream: Mutex<Option<Arc<StreamHandle>>>,
    closed: AtomicBool,
    watcher: Mutex<Option<RepoWatcher>>,
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

    pub fn repo(&self) -> Arc<Mutex<git2::Repository>> {
        self.repo.clone()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    /// Register a new log stream, cancelling any previous one.
    pub fn begin_log_stream(&self) -> Arc<StreamHandle> {
        install_stream(&self.log_stream)
    }

    /// Register a new file-history stream, cancelling any previous one.
    pub fn begin_history_stream(&self) -> Arc<StreamHandle> {
        install_stream(&self.history_stream)
    }

    /// Tear down: mark closed, cancel streams, stop the watcher. Idempotent.
    fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        if let Some(token) = self.log_stream.lock().take() {
            token.cancel();
        }
        if let Some(token) = self.history_stream.lock().take() {
            token.cancel();
        }
        // M2 op queue: stop accepting new ops; the worker drains its
        // backlog (jobs still resolve) and then exits.
        self.ops.shutdown();
        // Dropping the watcher joins its debounce thread.
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
            if slot.is_none() {
                let app = app.clone();
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
                ));
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
            repo: Arc::new(Mutex::new(repo)),
            generation: Arc::new(AtomicU64::new(0)),
            engine: default_engine(),
            m3: m3_engine(),
            m12: m12_engine(),
            lanes: Mutex::new(LaneState::default()),
            log_stream: Mutex::new(None),
            history_stream: Mutex::new(None),
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
                handle.shutdown();
                true
            }
            None => false,
        }
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
        static BOOTSTRAP: std::sync::Mutex<()> = std::sync::Mutex::new(());
        if fixtures_basic().join(".git").exists() {
            return;
        }
        let _guard = BOOTSTRAP.lock().unwrap();
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
    fn new_stream_cancels_previous_and_close_cancels_all() {
        ensure_fixtures();
        let mgr = RepoManager::new();
        let info = mgr.open_unwatched(&fixtures_basic()).unwrap();
        let handle = mgr.get(&info.repo_id).unwrap();

        let first = handle.begin_log_stream();
        assert!(!first.is_cancelled());
        let second = handle.begin_log_stream();
        assert!(first.is_cancelled(), "new log stream cancels the previous");
        assert!(!second.is_cancelled());

        // The history slot is an independent stream kind.
        let history = handle.begin_history_stream();
        assert!(!second.is_cancelled());
        assert!(!history.is_cancelled());

        mgr.close(&info.repo_id);
        assert!(second.is_cancelled());
        assert!(history.is_cancelled());
        assert!(handle.is_closed());
    }
}
