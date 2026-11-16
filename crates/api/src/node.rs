use parameters_core::{Definition, ValueType};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSummary {
    pub type_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub version: i32,
    pub hash: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeRecord {
    #[serde(flatten)]
    pub summary: NodeSummary,
    pub definition: Definition,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ImportStatus {
    New,
    Identical,
    Conflict { suggested: String },
    BuiltinClash { suggested: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportItem {
    pub name: String,
    #[serde(flatten)]
    pub status: ImportStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", content = "to", rename_all = "snake_case")]
pub enum Resolution {
    Rename(String),
    UseExisting,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Builtin,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortInfo {
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeInfo {
    pub type_name: String,
    pub kind: NodeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub privileged: bool,
    pub inputs: Vec<PortInfo>,
    pub outputs: Vec<PortInfo>,
    pub example: String,
}
