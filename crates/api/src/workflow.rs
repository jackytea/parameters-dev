use std::collections::BTreeMap;

use parameters_core::Workflow;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Ui,
    Mcp,
    Api,
    Webhook,
    Schedule,
}

impl Via {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Mcp => "mcp",
            Self::Api => "api",
            Self::Webhook => "webhook",
            Self::Schedule => "schedule",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::Ui,
            Self::Mcp,
            Self::Api,
            Self::Webhook,
            Self::Schedule,
        ]
        .into_iter()
        .find(|via| via.name() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Text,
    Json,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "format", content = "content", rename_all = "lowercase")]
pub enum Source {
    Text(String),
    Json(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowSummary {
    pub id: Uuid,
    pub name: String,
    pub version: i32,
    pub updated_via: Via,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkflowRecord {
    #[serde(flatten)]
    pub summary: WorkflowSummary,
    pub hash: String,
    pub document: Workflow,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedWorkflow {
    #[serde(flatten)]
    pub record: WorkflowRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub added_nodes: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub renamed_nodes: BTreeMap<String, String>,
}
