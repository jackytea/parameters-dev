use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::workflow::Via;

macro_rules! named {
    ($name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(rename_all = "lowercase")]
        pub enum $name {
            $($variant),+
        }

        impl $name {
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $text),+
                }
            }

            #[must_use]
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

named!(RunState {
    Queued => "queued",
    Running => "running",
    Succeeded => "succeeded",
    Failed => "failed",
    Cancelled => "cancelled",
});

named!(StepState {
    Running => "running",
    Retrying => "retrying",
    Succeeded => "succeeded",
    Failed => "failed",
    Skipped => "skipped",
    Cancelled => "cancelled",
});

impl RunState {
    #[must_use]
    pub const fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: Uuid,
    pub workflow_id: Option<Uuid>,
    pub workflow_name: String,
    pub status: RunState,
    pub trigger: Via,
    pub attempts: i32,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    #[serde(flatten)]
    pub summary: RunSummary,
    pub hash: String,
    pub inputs: Map<String, Value>,
    pub outputs: Map<String, Value>,
    pub errors: Vec<String>,
    pub cancel_requested: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub path: String,
    pub status: StepState,
    pub attempt: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub outputs: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogLine {
    pub seq: i64,
    pub path: String,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunUpdate {
    pub run: RunRecord,
    pub steps: Vec<StepRecord>,
    pub logs: Vec<LogLine>,
    pub cursor: i64,
}
