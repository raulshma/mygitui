//! Single-instance forwarding (`mygitui <path>` always lands in the
//! running window).
//!
//! The first process binds a per-user Unix-domain socket next to the app
//! data dir; every later launch connects to it, forwards its argv as one
//! JSON line and exits, and the primary publishes the received argv as
//! [`BackendEvent::CliArgs`](crate::service::events::BackendEvent::CliArgs)
//! on the backend bus, where the shell opens the requested repository.
//!
//! Platform scope: Unix only for now. Windows needs named pipes
//! (`tokio::net::windows::named_pipe`); until that lands, second launches
//! on Windows run standalone. A stale socket file (crashed previous
//! instance) is detected by the failed connect and removed before bind.

use std::path::PathBuf;

use crate::service::events::{global, BackendEvent};

/// Platform listener for the primary's accept loop: a Unix-domain socket
/// listener on Unix, nothing on Windows yet.
#[cfg(unix)]
pub type InstanceListener = tokio::net::UnixListener;
#[cfg(not(unix))]
pub type InstanceListener = ();

/// Cloneable one-shot handle for handing the listener from `run()` through
/// the iced boot closure (`Fn`, so it must clone, not move).
pub type SharedListener = std::sync::Arc<std::sync::Mutex<Option<InstanceListener>>>;

/// Takes the listener out of the shared handle (once).
pub fn take_listener(shared: &SharedListener) -> Option<InstanceListener> {
    shared.lock().ok().and_then(|mut guard| guard.take())
}

/// What [`bind_or_forward`] decided for this launch.
pub enum Role {
    /// This process is the primary; the listener (when the platform
    /// supports forwarding) accepts argv from later launches.
    Primary(Option<InstanceListener>),
    /// A primary is already running and the argv was forwarded; the caller
    /// should exit without starting a UI.
    Forwarded,
}

/// Per-user socket path, derived from the same data dir the config lives
/// in, so any second launch agrees on it without reading config.
fn socket_path() -> Option<PathBuf> {
    dirs::data_dir().map(|dir| dir.join("mygitui").join("instance.sock"))
}

/// Decide this launch's role: connect to a live primary and forward the
/// argv (`Forwarded`), or bind the socket ourselves (`Primary`).
#[cfg(unix)]
pub async fn bind_or_forward(args: &[String]) -> Role {
    let Some(path) = socket_path() else {
        return Role::Primary(None);
    };

    // A successful connect proves a live primary is listening.
    if let Ok(mut stream) = tokio::net::UnixStream::connect(&path).await {
        use tokio::io::AsyncWriteExt;
        let payload = serde_json::to_vec(&args).unwrap_or_default();
        if stream.write_all(&payload).await.is_ok() && stream.write_all(b"\n").await.is_ok() {
            let _ = stream.shutdown().await;
            return Role::Forwarded;
        }
        // Half-broken peer: fall through and take over the socket.
        tracing::warn!("instance socket connect succeeded but the write failed");
    }

    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Stale socket from a crashed instance: bind after removing it. A bind
    // failure with a live peer is a race between two primaries — log and
    // run standalone rather than exit (both windows stay usable).
    let _ = std::fs::remove_file(&path);
    match tokio::net::UnixListener::bind(&path) {
        Ok(listener) => Role::Primary(Some(listener)),
        Err(err) => {
            tracing::warn!(%err, "single-instance bind failed; running standalone");
            Role::Primary(None)
        }
    }
}

/// Windows placeholder: no forwarding yet — every launch is standalone.
#[cfg(not(unix))]
pub async fn bind_or_forward(_args: &[String]) -> Role {
    Role::Primary(None)
}

/// Accept loop for the primary: each connection carries one JSON argv line;
/// forward it to the UI through the backend event bus. Runs forever; the
/// process exits elsewhere.
pub async fn serve(listener: InstanceListener) {
    use tokio::io::AsyncBufReadExt;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                let mut lines = tokio::io::BufReader::new(stream).lines();
                match lines.next_line().await {
                    Ok(Some(line)) => match serde_json::from_str::<Vec<String>>(&line) {
                        Ok(args) => {
                            global().publish(BackendEvent::CliArgs(args));
                        }
                        Err(err) => tracing::debug!(%err, "malformed instance forward"),
                    },
                    Ok(None) | Err(_) => continue,
                }
            }
            Err(err) => {
                tracing::warn!(%err, "instance listener failed");
                break;
            }
        }
    }
}

