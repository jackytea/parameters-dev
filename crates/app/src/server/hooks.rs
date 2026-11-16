use dioxus::server::axum::body::Bytes;
use dioxus::server::axum::extract::{DefaultBodyLimit, Path, State};
use dioxus::server::axum::response::{IntoResponse, Response};
use dioxus::server::axum::routing::post;
use dioxus::server::axum::{Json, Router};
use dioxus::server::http::{HeaderMap, StatusCode};
use parameters_service::{MAX_PAYLOAD_BYTES, Service};
use serde_json::json;

const SIGNATURE_HEADERS: [&str; 2] = ["x-parameters-signature", "x-hub-signature-256"];

pub(crate) fn routes(service: Service) -> Router {
    Router::new()
        .route("/hooks/{token}", post(fire))
        .layer(DefaultBodyLimit::max(MAX_PAYLOAD_BYTES))
        .with_state(service)
}

async fn fire(
    State(service): State<Service>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let signature = SIGNATURE_HEADERS
        .iter()
        .find_map(|name| headers.get(*name)?.to_str().ok());
    match service.fire_webhook(&token, &body, signature).await {
        Ok(run) => (
            StatusCode::ACCEPTED,
            Json(json!({ "run": run.id, "status": run.status })),
        )
            .into_response(),
        Err(error) => (
            StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error),
        )
            .into_response(),
    }
}
