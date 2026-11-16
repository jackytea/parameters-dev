use serde::{Deserialize, Serialize};

pub const MAX_PAGE_SIZE: u32 = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
