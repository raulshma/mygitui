//! Process-wide tokio runtime.
//!
//! Every async service body (`service::*`) and spawn_blocking hop runs
//! here, independent of the UI event loop (iced drives its own executor for
//! `Task::perform`; op-queue workers and blocking engine hops run on this
//! runtime). Built lazily on first use so unit tests never pay for an
//! unused runtime.

use std::future::Future;
use std::sync::OnceLock;

static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// The shared multi-thread runtime. Panics only if the OS refuses to create
/// any worker thread (nothing sane can run then).
pub fn runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("mygitui")
            .build()
            .expect("build mygitui tokio runtime")
    })
}

/// Spawn a future on the global runtime.
pub fn spawn<F>(future: F) -> tokio::task::JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    runtime().spawn(future)
}

/// Run blocking work (git2 calls, FS traversal) on the blocking pool.
pub async fn spawn_blocking<F, R>(f: F) -> Result<R, tokio::task::JoinError>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(f).await
}

/// Drain a future to completion. For synchronous entry points that must call
/// async code (never from inside another runtime worker).
pub fn block_on<F: Future>(future: F) -> F::Output {
    runtime().block_on(future)
}
