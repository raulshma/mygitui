//! Backend → UI event bus.
//!
//! Every asynchronous signal the engine, watcher, op queue, auth broker,
//! remote layer, PTY and action runners produce goes through one in-process
//! broadcast channel. The iced app subscribes once (`ui::subscription`) and
//! routes each event into its `Message` enum; tests and headless callers can
//! subscribe too.
//!
//! Delivery is best-effort: with no live subscriber the event is dropped
//! (`publish` reports `false`). Streams that must stop when the consumer
//! goes away (log pages) treat a no-subscriber publish as cancellation.

use std::sync::OnceLock;

use tokio::sync::broadcast;

use crate::actions::ActionOutput;
use crate::auth::AuthRequestEvent;
use crate::ops::{OpLog, OpProgress};
use crate::pty::{PtyExit, PtyOutput};
use crate::remote::prompts::{DeviceAuthRequestEvent, HostKeyRequestEvent};
use crate::remote::transport::DeviceStatusEvent;
use crate::service::log::LogPage;
use crate::service::repo::CloneProgress;
use crate::watcher::RepoChangedEvent;

/// Bus capacity: burst-tolerant (a status storm after a big checkout) while
/// still bounding memory; slow consumers lose the oldest events, and every
/// consumer must be able to recover from a `Lagged` error.
pub const BUS_CAPACITY: usize = 4096;

/// One backend event, published by every async backend surface (queues,
/// watchers, brokers, streams, PTY and action runners).
#[derive(Debug, Clone)]
pub enum BackendEvent {
    /// FS watcher batch or remote poll tick (drives every panel refresh).
    RepoChanged(RepoChangedEvent),
    /// Throttled op progress and the final `done` event.
    OpProgress(OpProgress),
    /// Raw op output line (hook/CLI stderr/stdout).
    OpLog(OpLog),
    /// Net-op credential prompt; answer via `service::network::auth_respond`.
    AuthRequest(AuthRequestEvent),
    /// First-contact / changed SSH host key prompt.
    HostKeyRequest(HostKeyRequestEvent),
    /// SSH secret prompt (key passphrase / device password).
    DeviceAuthRequest(DeviceAuthRequestEvent),
    /// Device connection status transition.
    DeviceStatus(DeviceStatusEvent),
    /// Clone transfer progress.
    CloneProgress(CloneProgress),
    /// One page of a log / file-history stream. `request_id` is minted by
    /// the UI per request so superseded streams can be discarded.
    LogPage { request_id: u64, page: LogPage },
    /// One line (or the final event) of a custom action run.
    ActionOutput(ActionOutput),
    /// One coalesced PTY output window.
    PtyOutput(PtyOutput),
    /// PTY session ended.
    PtyExit(PtyExit),
    /// Second-launch CLI args forwarded by the single-instance guard.
    CliArgs(Vec<String>),
}

/// Broadcast hub for [`BackendEvent`]s.
pub struct EventBus {
    tx: broadcast::Sender<BackendEvent>,
}

impl EventBus {
    fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// Publish one event. Returns `false` when nobody is subscribed (the
    /// event had nowhere to go and was dropped).
    pub fn publish(&self, event: BackendEvent) -> bool {
        self.tx.send(event).is_ok()
    }

    /// New consumer; each event is delivered to every receiver.
    pub fn subscribe(&self) -> broadcast::Receiver<BackendEvent> {
        self.tx.subscribe()
    }

    /// Number of live receivers.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

static BUS: OnceLock<EventBus> = OnceLock::new();

/// Process-wide bus. Created on first use so every producer (queues,
/// watchers, brokers) shares one channel even before the UI starts.
pub fn global() -> &'static EventBus {
    BUS.get_or_init(EventBus::new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::RepoId;

    fn changed(repo: &str, full: bool) -> BackendEvent {
        BackendEvent::RepoChanged(RepoChangedEvent {
            repo_id: RepoId(repo.to_string()),
            paths: vec!["a.txt".to_string()],
            head_moved: false,
            full,
        })
    }

    #[test]
    fn publish_without_subscribers_reports_false() {
        let bus = EventBus::new();
        assert!(!bus.publish(changed("r", false)));
    }

    #[test]
    fn every_receiver_gets_every_event_in_order() {
        let bus = EventBus::new();
        let mut a = bus.subscribe();
        let mut b = bus.subscribe();
        assert!(bus.publish(changed("r", false)));
        assert!(bus.publish(changed("r", true)));
        assert_eq!(bus.receiver_count(), 2);
        for rx in [&mut a, &mut b] {
            let first = rx.try_recv().expect("first event");
            let second = rx.try_recv().expect("second event");
            match (first, second) {
                (BackendEvent::RepoChanged(a), BackendEvent::RepoChanged(b)) => {
                    assert_eq!(a.repo_id.0, "r");
                    assert!(!a.full);
                    assert!(b.full);
                }
                other => panic!("unexpected events: {other:?}"),
            }
        }
    }

    #[test]
    fn dropped_receiver_does_not_break_publishing() {
        let bus = EventBus::new();
        {
            let _gone = bus.subscribe();
        }
        assert!(!bus.publish(changed("r", false)));
        let mut rx = bus.subscribe();
        assert!(bus.publish(changed("r", false)));
        assert!(rx.try_recv().is_ok());
    }
}
