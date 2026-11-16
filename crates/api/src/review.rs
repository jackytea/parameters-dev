use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrivilegedStep {
    pub path: String,
    pub uses: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub workflow_id: Uuid,
    pub hash: String,
    pub approved: bool,
    pub privileged: Vec<PrivilegedStep>,
    pub hosts: Vec<String>,
    pub dynamic_urls: Vec<String>,
    pub secrets: Vec<String>,
}

impl Review {
    #[must_use]
    pub fn needs_approval(&self) -> bool {
        !self.privileged.is_empty() && !self.approved
    }
}
