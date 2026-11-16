use parameters_store::Notification;
use tokio::sync::{Notify, broadcast};

const CAPACITY: usize = 1_024;

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
