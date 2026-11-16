use serde::{Deserialize, Serialize};

use crate::problem::Problem;
use crate::review::Review;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiError {
    #[error("{message}")]
    Invalid {
        message: String,
        problems: Vec<Problem>,
    },
    #[error("{message}")]
    BadRequest { message: String },
    #[error("{message}")]
    NotFound { message: String },
    #[error("{message}")]
    Conflict {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current_version: Option<i32>,
    },
    #[error("{message}")]
    ApprovalRequired {
        message: String,
        review: Box<Review>,
    },
    #[error("{message}")]
    Internal { message: String },
}

impl ApiError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::BadRequest {
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::NotFound {
            message: message.into(),
        }
    }

    #[must_use]
    pub fn internal() -> Self {
        Self::Internal {
            message: "something went wrong on the server; the details are in the server log".into(),
        }
    }

    #[must_use]
    pub const fn status(&self) -> u16 {
        match self {
            Self::Invalid { .. } => 422,
            Self::BadRequest { .. } => 400,
            Self::NotFound { .. } => 404,
            Self::Conflict { .. } => 409,
            Self::ApprovalRequired { .. } => 403,
            Self::Internal { .. } => 500,
        }
    }
}
