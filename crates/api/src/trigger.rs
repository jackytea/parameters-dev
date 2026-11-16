use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TriggerKind {
    Webhook {
        signed: bool,
    },
    Schedule {
        cron: String,
        #[serde(default, with = "time::serde::rfc3339::option")]
        next_run_at: Option<OffsetDateTime>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerSummary {
    pub id: Uuid,
    pub workflow_id: Uuid,
    pub name: String,
    #[serde(flatten)]
    pub kind: TriggerKind,
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub inputs: Map<String, Value>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub last_fired_at: Option<OffsetDateTime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CreatedTrigger {
    #[serde(flatten)]
    pub summary: TriggerSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signing_secret: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum NewTrigger {
    Webhook { signed: bool },
    Schedule { cron: String },
}
