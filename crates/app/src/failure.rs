use std::fmt;

use dioxus::fullstack::{AsStatusCode, ServerFnError, StatusCode};
use parameters_api::ApiError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
#[cfg_attr(
    not(feature = "server"),
    allow(
        dead_code,
        reason = "the client only creates it by deserializing server responses"
    )
)]
pub(crate) struct Failure(pub(crate) ApiError);

impl fmt::Display for Failure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for Failure {}

impl From<ApiError> for Failure {
    fn from(error: ApiError) -> Self {
        Self(error)
    }
}

impl From<ServerFnError> for Failure {
    fn from(error: ServerFnError) -> Self {
        let message = match &error {
            ServerFnError::ServerError { message, .. } => message.clone(),
            other => other.to_string(),
        };
        Self(if error.as_status_code().is_client_error() {
            ApiError::BadRequest { message }
        } else {
            ApiError::Internal { message }
        })
    }
}

impl AsStatusCode for Failure {
    fn as_status_code(&self) -> StatusCode {
        StatusCode::from_u16(self.0.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_status_and_maps_transport_errors() {
        let conflict = Failure(ApiError::Conflict {
            message: "changed".into(),
            current_version: Some(2),
        });
        assert_eq!(conflict.as_status_code(), StatusCode::CONFLICT);
        assert_eq!(
            serde_json::to_value(&conflict).unwrap(),
            serde_json::json!({"kind": "conflict", "message": "changed", "current_version": 2})
        );
        let rejected = Failure::from(ServerFnError::ServerError {
            message: "missing field `source`".into(),
            code: 422,
            details: None,
        });
        assert!(matches!(rejected.0, ApiError::BadRequest { .. }));
        assert_eq!(rejected.as_status_code(), StatusCode::BAD_REQUEST);
    }
}
