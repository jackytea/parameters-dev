use sqlx::postgres::PgListener;
use sqlx::{Executor, Postgres};
use uuid::Uuid;

use crate::{Result, Store};

pub(crate) const CHANNEL: &str = "parameters";
const QUEUE: &str = "queue";
const RUN: &str = "run:";
const CANCEL: &str = "cancel:";
const WORKFLOW: &str = "workflow:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notification {
    Queued,
    RunChanged(Uuid),
    CancelRequested(Uuid),
    WorkflowChanged(Uuid),
    Resync,
}

impl Notification {
    fn payload(self) -> String {
        match self {
            Self::Queued | Self::Resync => QUEUE.to_owned(),
            Self::RunChanged(id) => format!("{RUN}{id}"),
            Self::CancelRequested(id) => format!("{CANCEL}{id}"),
            Self::WorkflowChanged(id) => format!("{WORKFLOW}{id}"),
        }
    }

    fn parse(payload: &str) -> Option<Self> {
        if payload == QUEUE {
            return Some(Self::Queued);
        }
        if let Some(id) = payload.strip_prefix(RUN) {
            return Uuid::try_parse(id).ok().map(Self::RunChanged);
        }
        if let Some(id) = payload.strip_prefix(WORKFLOW) {
            return Uuid::try_parse(id).ok().map(Self::WorkflowChanged);
        }
        payload
            .strip_prefix(CANCEL)
            .and_then(|id| Uuid::try_parse(id).ok())
            .map(Self::CancelRequested)
    }
}

pub(crate) async fn send<'e>(
    executor: impl Executor<'e, Database = Postgres>,
    notification: Notification,
) -> Result<()> {
    sqlx::query!("SELECT pg_notify($1, $2)", CHANNEL, notification.payload())
        .execute(executor)
        .await?;
    Ok(())
}

pub struct Listener(PgListener);

impl Listener {
    pub async fn recv(&mut self) -> Result<Notification> {
        loop {
            match self.0.try_recv().await? {
                None => return Ok(Notification::Resync),
                Some(message) => {
                    if let Some(notification) = Notification::parse(message.payload()) {
                        return Ok(notification);
                    }
                }
            }
        }
    }
}

impl Store {
    pub async fn listen(&self) -> Result<Listener> {
        let mut listener = PgListener::connect_with(&self.pool).await?;
        listener.listen(CHANNEL).await?;
        Ok(Listener(listener))
    }

    pub async fn notify(&self, notification: Notification) -> Result<()> {
        send(&self.pool, notification).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payloads_round_trip() {
        let id = Uuid::from_u128(42);
        for notification in [
            Notification::Queued,
            Notification::RunChanged(id),
            Notification::CancelRequested(id),
            Notification::WorkflowChanged(id),
        ] {
            assert_eq!(
                Notification::parse(&notification.payload()),
                Some(notification)
            );
        }
        assert_eq!(Notification::parse("run:nope"), None);
        assert_eq!(Notification::parse("other"), None);
    }
}
