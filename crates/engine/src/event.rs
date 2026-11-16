use std::time::Duration;

use serde::Serialize;

use crate::node::Values;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    Succeeded,
    Failed,
    Skipped,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RunStatus {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    StepStarted {
        path: String,
        attempt: u32,
    },
    StepRetrying {
        path: String,
        attempt: u32,
        error: String,
        delay: Duration,
    },
    StepFinished(StepReport),
    Log {
        path: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepReport {
    pub path: String,
    pub status: StepStatus,
    pub attempts: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Values::is_empty")]
    pub outputs: Values,
    #[serde(serialize_with = "milliseconds")]
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunReport {
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    pub steps: Vec<StepReport>,
    pub outputs: Values,
    #[serde(serialize_with = "milliseconds")]
    pub duration: Duration,
}

impl RunReport {
    #[must_use]
    pub fn step(&self, path: &str) -> Option<&StepReport> {
        self.steps.iter().find(|step| step.path == path)
    }
}

fn milliseconds<S: serde::Serializer>(
    duration: &Duration,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_u128(duration.as_millis())
}
