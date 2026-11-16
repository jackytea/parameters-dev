use serde::{Deserialize, Serialize};

use crate::{Mode, NodeInfo};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerDocs {
    pub mode: Mode,
    pub nodes: Vec<NodeInfo>,
    pub tools: Vec<ToolDoc>,
    pub limits: Limits,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDoc {
    pub name: String,
    pub title: String,
    pub description: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub privileged_nodes: bool,
    pub private_network: bool,
    pub max_steps: Option<u64>,
    pub max_run_minutes: Option<u64>,
    pub max_active_runs: Option<i64>,
    pub requests_per_minute: Option<u32>,
    pub min_schedule_minutes: Option<u64>,
}
