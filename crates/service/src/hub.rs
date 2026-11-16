use std::time::Duration;

use parameters_store::Notification;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{Notify, broadcast};

const CAPACITY: usize = 1_024;
const QUIET_REFRESH: Duration = Duration::from_secs(30);

pub(crate) struct Hub {
    pub(crate) queue: Notify,
    runs: broadcast::Sender<Notification>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            queue: Notify::new(),
            runs: broadcast::channel(CAPACITY).0,
        }
    }
}

impl Hub {
    pub(crate) fn publish(&self, notification: Notification) {
        if matches!(notification, Notification::Queued | Notification::Resync) {
            self.queue.notify_waiters();
        }
        if notification != Notification::Queued {
            let _ = self.runs.send(notification);
        }
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Notification> {
        self.runs.subscribe()
    }
}

pub(crate) async fn changed(
    notifications: &mut broadcast::Receiver<Notification>,
    wanted: Notification,
) {
    let quiet = tokio::time::sleep(QUIET_REFRESH);
    tokio::pin!(quiet);
    loop {
        tokio::select! {
            () = &mut quiet => return,
            received = notifications.recv() => match received {
                Ok(notification) if notification == wanted => return,
                Ok(Notification::Resync) | Err(RecvError::Lagged(_)) => return,
                Ok(_) => {}
                Err(RecvError::Closed) => {
                    quiet.as_mut().await;
                    return;
                }
            },
        }
    }
}
