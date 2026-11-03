//! iced subscription bridging the backend event bus.
//!
//! One stream feeds every [`BackendEvent`] into the app's `Message` loop;
//! `Lagged` gaps are logged and skipped (consumers always recover by
//! refetching on the next event or the explicit refresh path).

use iced::futures::{SinkExt, Stream};
use tokio::sync::broadcast::error::RecvError;

use crate::service::events::{global, BackendEvent};

/// Stream of every event published on the process-wide bus.
pub fn stream() -> impl Stream<Item = BackendEvent> {
    iced::stream::channel::<BackendEvent>(
        256,
        |mut output: iced::futures::channel::mpsc::Sender<BackendEvent>| async move {
            let mut rx = global().subscribe();
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if output.send(event).await.is_err() {
                            break;
                        }
                    }
                    Err(RecvError::Lagged(skipped)) => {
                        tracing::warn!(skipped, "UI event stream lagged; skipping events");
                    }
                    Err(RecvError::Closed) => break,
                }
            }
        },
    )
}
