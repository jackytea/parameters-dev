use std::fmt::Write;
use std::net::SocketAddr;

use dioxus::server::axum::extract::{ConnectInfo, Path, Request, State};
use dioxus::server::axum::middleware::Next;
use dioxus::server::axum::response::{AppendHeaders, Html, IntoResponse, Response};
use dioxus::server::axum::routing::{get, post};
use dioxus::server::axum::{Extension, Form, Json, Router};
use dioxus::server::http::header::{
    AUTHORIZATION, COOKIE, LOCATION, ORIGIN, SET_COOKIE, WWW_AUTHENTICATE,
};
use dioxus::server::http::{HeaderMap, Method, StatusCode};
use parameters_api::{ApiError, Via};
use parameters_service::{Actor, Service, TOKEN_PREFIX, safe_next};
use serde::Deserialize;
use serde_json::json;

const SESSION_COOKIE: &str = "__Host-parameters";
const PLAIN_SESSION_COOKIE: &str = "parameters_session";
const OAUTH_COOKIE: &str = "parameters_oauth";
const SESSION_SECONDS: u32 = 30 * 24 * 60 * 60;
const OAUTH_SECONDS: u32 = 10 * 60;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClientAddress(pub(crate) String);

fn secure(service: &Service) -> bool {
    service
        .public_url()
        .is_some_and(|url| url.starts_with("https://"))
}

fn session_cookie(service: &Service) -> &'static str {
    if secure(service) {
        SESSION_COOKIE
    } else {
        PLAIN_SESSION_COOKIE
    }
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}

fn set_cookie(service: &Service, name: &str, value: &str, path: &str, seconds: u32) -> String {
    let secure = if secure(service) { "; Secure" } else { "" };
    format!("{name}={value}; Path={path}; HttpOnly; SameSite=Lax; Max-Age={seconds}{secure}")
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn client_address(request: &Request, trust_proxy: bool) -> String {
    let forwarded = trust_proxy
        .then(|| {
            request
                .headers()
                .get("x-forwarded-for")?
                .to_str()
                .ok()?
                .rsplit(',')
                .next()
                .map(|address| address.trim().to_owned())
        })
        .flatten();
    forwarded
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|info| info.0.ip().to_string())
        })
        .unwrap_or_default()
}

fn same_origin(request: &Request, public_url: Option<&str>) -> bool {
    let origin = request
        .headers()
        .get(ORIGIN)
        .and_then(|origin| origin.to_str().ok());
    matches!((origin, public_url), (Some(origin), Some(expected)) if origin == expected)
}

fn refuse(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

pub(crate) async fn authenticate(
    State(service): State<Service>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    let mcp = path == "/mcp" || path.starts_with("/mcp/");
    let address = client_address(&request, service.trusts_proxy());
    let mut by_session = false;
    let actor = if !service.is_managed() {
        Some(Actor::local(if mcp { Via::Mcp } else { Via::Ui }))
    } else if let Some(token) = bearer(request.headers()) {
        if token.starts_with(TOKEN_PREFIX) {
            service
                .token_actor(token, if mcp { Via::Mcp } else { Via::Api })
                .await
        } else if mcp {
            None
        } else {
            service.session_actor(token).await
        }
    } else if mcp {
        None
    } else if let Some(token) = cookie(request.headers(), session_cookie(&service)) {
        by_session = true;
        service.session_actor(token).await
    } else {
        None
    };
    if mcp && actor.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            [(WWW_AUTHENTICATE, "Bearer realm=\"parameters\"")],
            "Connect with an access token from your Parameters account",
        )
            .into_response();
    }
    let unsafe_method = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if by_session && unsafe_method && !same_origin(&request, service.public_url()) {
        return refuse(
            StatusCode::FORBIDDEN,
            "This request did not come from Parameters",
        );
    }
    let key = actor.map_or_else(
        || format!("address:{address}"),
        |actor| format!("user:{}", actor.user),
    );
    if !service.allow_request(&key) {
        return refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many requests; slow down",
        );
    }
    request.extensions_mut().insert(ClientAddress(address));
    if let Some(actor) = actor {
        request.extensions_mut().insert(actor);
    }
    next.run(request).await
}

pub(crate) fn routes(service: Service) -> Router {
    Router::new()
        .route("/auth/email", get(confirm_email).post(finish_email))
        .route("/auth/signout", post(sign_out))
        .route("/auth/device", post(start_device))
        .route("/auth/device/token", post(claim_device))
        .route("/auth/{provider}", get(start_oauth))
        .route("/auth/{provider}/callback", get(finish_oauth))
        .with_state(service)
}

fn escape(text: &str) -> String {
    text.chars()
        .fold(String::with_capacity(text.len()), |mut out, character| {
            match character {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                other => out.push(other),
            }
            out
        })
}

fn page(title: &str, body: &str) -> Html<String> {
    Html(format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{title}</title><style>body{{font-family:system-ui,sans-serif;max-width:32rem;margin:15vh auto;padding:0 16px;line-height:1.5}}button{{font:inherit;padding:8px 16px;border:1px solid #2563eb;background:#2563eb;color:#fff;cursor:pointer}}</style></head><body><h1>{title}</h1>{body}</body></html>",
        title = escape(title)
    ))
}

fn problem(error: &ApiError) -> Response {
    let status = StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (
        status,
        page(
            "Could not sign in",
            &format!(
                "<p>{}</p><p><a href=\"/signin\">Try again</a></p>",
                escape(&error.to_string())
            ),
        ),
    )
        .into_response()
}

fn hex(text: &str) -> String {
    text.bytes()
        .fold(String::with_capacity(text.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn unhex(text: &str) -> Option<String> {
    let bytes = (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(text.get(index..index + 2)?, 16).ok())
        .collect::<Option<Vec<u8>>>()?;
    String::from_utf8(bytes).ok()
}

fn signed_in(service: &Service, session: &str, next: &str, extra: Option<String>) -> Response {
    let mut cookies = vec![(
        SET_COOKIE,
        set_cookie(
            service,
            session_cookie(service),
            session,
            "/",
            SESSION_SECONDS,
        ),
    )];
    cookies.extend(extra.map(|cookie| (SET_COOKIE, cookie)));
    (
        StatusCode::SEE_OTHER,
        [(LOCATION, next.to_owned())],
        AppendHeaders(cookies),
    )
        .into_response()
}

#[derive(Deserialize)]
struct TokenForm {
    token: String,
}

async fn confirm_email(Form(form): Form<TokenForm>) -> Html<String> {
    page(
        "Sign in to Parameters",
        &format!(
            "<form method=\"post\" action=\"/auth/email\"><input type=\"hidden\" name=\"token\" value=\"{}\"><button type=\"submit\">Sign in</button></form>",
            escape(&form.token)
        ),
    )
}

async fn finish_email(State(service): State<Service>, Form(form): Form<TokenForm>) -> Response {
    match service.finish_email_sign_in(&form.token).await {
        Ok(signed) => signed_in(&service, &signed.session, &signed.next, None),
        Err(error) => problem(&error),
    }
}

#[derive(Deserialize)]
struct NextForm {
    next: Option<String>,
}

async fn start_oauth(
    State(service): State<Service>,
    Path(provider): Path<String>,
    Form(form): Form<NextForm>,
) -> Response {
    match service.start_oauth(&provider) {
        Ok(start) => {
            let next = safe_next(form.next.as_deref()).unwrap_or_else(|| "/".into());
            let value = format!(
                "{provider}.{}.{}.{}",
                start.state,
                start.verifier,
                hex(&next)
            );
            (
                StatusCode::SEE_OTHER,
                [
                    (LOCATION, start.url),
                    (
                        SET_COOKIE,
                        set_cookie(&service, OAUTH_COOKIE, &value, "/auth", OAUTH_SECONDS),
                    ),
                ],
            )
                .into_response()
        }
        Err(error) => problem(&error),
    }
}

#[derive(Deserialize)]
struct Callback {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn finish_oauth(
    State(service): State<Service>,
    Path(provider): Path<String>,
    headers: HeaderMap,
    Form(callback): Form<Callback>,
) -> Response {
    let refused = |message: &str| {
        problem(&ApiError::Unauthorized {
            message: message.to_owned(),
        })
    };
    if callback.error.is_some() {
        return refused("the sign-in was cancelled");
    }
    let expected = cookie(&headers, OAUTH_COOKIE).and_then(|value| {
        let mut parts = value.splitn(4, '.');
        Some((parts.next()?, parts.next()?, parts.next()?, parts.next()?))
    });
    let (Some((cookie_provider, state, verifier, next)), Some(code), Some(returned)) = (
        expected,
        callback.code.as_deref(),
        callback.state.as_deref(),
    ) else {
        return refused("the sign-in expired; start again");
    };
    if cookie_provider != provider || state != returned {
        return refused("the sign-in could not be verified; start again");
    }
    let clear = set_cookie(&service, OAUTH_COOKIE, "", "/auth", 0);
    match service.finish_oauth(&provider, code, verifier).await {
        Ok(session) => {
            let next = safe_next(unhex(next).as_deref()).unwrap_or_else(|| "/".into());
            signed_in(&service, &session, &next, Some(clear))
        }
        Err(error) => problem(&error),
    }
}

async fn sign_out(State(service): State<Service>, headers: HeaderMap) -> Response {
    let name = session_cookie(&service);
    let token = cookie(&headers, name)
        .or_else(|| bearer(&headers).filter(|token| !token.starts_with(TOKEN_PREFIX)));
    if let Some(token) = token {
        let _ = service.sign_out(token).await;
    }
    (
        StatusCode::SEE_OTHER,
        [
            (LOCATION, "/signin".to_owned()),
            (SET_COOKIE, set_cookie(&service, name, "", "/", 0)),
        ],
    )
        .into_response()
}

#[derive(Deserialize)]
struct DeviceRequest {
    #[serde(default)]
    name: String,
}

#[derive(Deserialize)]
struct DeviceClaim {
    device_code: String,
}

fn failure(error: &ApiError) -> Response {
    (
        StatusCode::from_u16(error.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        Json(error),
    )
        .into_response()
}

async fn start_device(
    State(service): State<Service>,
    Extension(address): Extension<ClientAddress>,
    Json(request): Json<DeviceRequest>,
) -> Response {
    match service.start_device(&request.name, &address.0).await {
        Ok(start) => Json(start).into_response(),
        Err(error) => failure(&error),
    }
}

async fn claim_device(State(service): State<Service>, Json(claim): Json<DeviceClaim>) -> Response {
    match service.claim_device(&claim.device_code).await {
        Ok(Some(token)) => Json(json!({ "token": token })).into_response(),
        Ok(None) => (StatusCode::ACCEPTED, Json(json!({ "pending": true }))).into_response(),
        Err(error) => failure(&error),
    }
}

#[cfg(test)]
mod tests {
    use dioxus::server::http::HeaderValue;

    use super::*;

    #[test]
    fn reads_cookies_and_bearer_tokens() {
        let mut headers = HeaderMap::new();
        headers.insert(
            COOKIE,
            HeaderValue::from_static("theme=dark; __Host-parameters=abc; other=1"),
        );
        headers.insert(AUTHORIZATION, HeaderValue::from_static("Bearer prm_xyz"));
        assert_eq!(cookie(&headers, "__Host-parameters"), Some("abc"));
        assert_eq!(cookie(&headers, "missing"), None);
        assert_eq!(bearer(&headers), Some("prm_xyz"));
        assert_eq!(
            escape("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
        assert_eq!(
            unhex(&hex("/connect?code=AB-CD")).as_deref(),
            Some("/connect?code=AB-CD")
        );
        assert_eq!(unhex("zz"), None);
    }
}
