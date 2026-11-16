mod accounts;
mod cursor;
mod nodes;
mod notify;
mod queue;
mod runs;
mod secrets;
mod triggers;
mod workflows;

use std::time::Duration;

use sqlx::migrate::Migrator;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use uuid::Uuid;

pub use accounts::NewToken;
pub use cursor::Cursor;
pub use nodes::{LibraryChange, StoredNode};
pub use notify::{Listener, Notification};
pub use queue::{ClaimedRun, LogWrite, StepWrite};
pub use runs::{NewRun, RunChanges};
pub use secrets::EncryptedSecret;
pub use sqlx::postgres::PgConnectOptions as ConnectOptions;
pub use triggers::{FiringTrigger, TriggerRecord, WebhookSecret};
pub use workflows::{NewWorkflow, StoredWorkflow, WorkflowUpdate};

pub static MIGRATOR: Migrator = sqlx::migrate!();

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("the database holds a value this version cannot read: {0}")]
    Corrupt(String),
}

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Versioned<T> {
    Done(T),
    Conflict(i32),
    Missing,
}

#[derive(Clone)]
pub struct Store {
    pool: PgPool,
}

impl Store {
    pub async fn connect(options: PgConnectOptions, max_connections: u32) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(options)
            .await?;
        Ok(Self { pool })
    }

    #[must_use]
    pub const fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn migrate(&self) -> Result<()> {
        MIGRATOR.run(&self.pool).await?;
        Ok(())
    }

    pub async fn ensure_user(&self, id: Uuid, display_name: &str) -> Result<()> {
        sqlx::query!(
            "INSERT INTO users (id, display_name) VALUES ($1, $2) ON CONFLICT (id) DO NOTHING",
            id,
            display_name
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn named<T>(value: &str, from_name: impl Fn(&str) -> Option<T>) -> Result<T> {
    from_name(value).ok_or_else(|| StoreError::Corrupt(format!("unknown value `{value}`")))
}

fn object(value: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match value {
        serde_json::Value::Object(object) => object,
        _ => serde_json::Map::new(),
    }
}
