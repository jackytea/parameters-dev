use std::path::PathBuf;
use std::time::{Duration, Instant};

use dioxus::fullstack::http::header::{AUTHORIZATION, CONTENT_TYPE};
use dioxus::fullstack::{
    HeaderMap, HeaderValue, clear_request_headers, get_server_url, reqwest, set_request_headers,
    set_server_url,
};
use parameters_api::DeviceStart;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub(crate) const DEFAULT_SERVER: &str = match option_env!("PARAMETERS_SERVER_URL") {
    Some(url) => url,
    None => "https://parameters.dev",
};
const SETTINGS_FILE: &str = "settings.json";
const SESSION_FILE: &str = "session";

#[derive(Debug, Default, Serialize, Deserialize)]
struct Settings {
    server: Option<String>,
}

fn directory() -> Option<PathBuf> {
    dirs::data_local_dir()
        .map(|directory| directory.join("Parameters"))
        .or_else(android_directory)
}

#[cfg(target_os = "android")]
fn android_directory() -> Option<PathBuf> {
    let command = std::fs::read_to_string("/proc/self/cmdline").ok()?;
    let package = command.split('\0').next()?.trim();
    (!package.is_empty()).then(|| PathBuf::from(format!("/data/data/{package}/files")))
}

#[cfg(not(target_os = "android"))]
const fn android_directory() -> Option<PathBuf> {
    None
}

fn path(name: &str) -> Option<PathBuf> {
    directory().map(|directory| directory.join(name))
}

fn authorize(token: &str) {
    let mut headers = HeaderMap::new();
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        headers.insert(AUTHORIZATION, value);
    }
    set_request_headers(headers);
}

pub(crate) fn configure() {
    let settings: Settings = path(SETTINGS_FILE)
        .and_then(|file| std::fs::read(file).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let server = settings.server.unwrap_or_else(|| DEFAULT_SERVER.to_owned());
    set_server_url(Box::leak(server.into_boxed_str()));
    if let Some(token) = path(SESSION_FILE)
        .and_then(|file| std::fs::read_to_string(file).ok())
        .map(|token| token.trim().to_owned())
        .filter(|token| !token.is_empty())
    {
        authorize(&token);
    }
}

pub(crate) fn server() -> &'static str {
    get_server_url()
}

async fn write_private(name: &str, contents: &str) -> Result<(), String> {
    let file = path(name).ok_or("this device has no place to keep settings")?;
    if let Some(parent) = file.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|error| error.to_string())?;
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut handle = options
        .open(&file)
        .await
        .map_err(|error| error.to_string())?;
    tokio::io::AsyncWriteExt::write_all(&mut handle, contents.as_bytes())
        .await
        .map_err(|error| error.to_string())
}

pub(crate) async fn save_server(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url.trim())
        .map_err(|_| "enter an address such as https://parameters.dev".to_owned())?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err("the address must start with http:// or https://".into());
    }
    let origin = parsed.origin().ascii_serialization();
    let settings = serde_json::to_string(&Settings {
        server: Some(origin),
    })
    .map_err(|error| error.to_string())?;
    write_private(SETTINGS_FILE, &settings).await?;
    if let Some(file) = path(SESSION_FILE) {
        let _ = tokio::fs::remove_file(file).await;
    }
    Ok(())
}

#[cfg(feature = "desktop")]
pub(crate) fn restart() {
    if let Ok(executable) = std::env::current_exe()
        && std::process::Command::new(executable).spawn().is_ok()
    {
        std::process::exit(0);
    }
}

async fn post(http: &reqwest::Client, path: &str, body: &Value) -> Result<(u16, Value), String> {
    let response = http
        .post(format!("{}{path}", server()))
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|error| format!("cannot reach {}: {error}", server()))?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.map_err(|error| error.to_string())?;
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

fn message(body: &Value) -> String {
    body["message"]
        .as_str()
        .unwrap_or("the server refused the sign-in")
        .to_owned()
}

pub(crate) async fn sign_in(mut show: impl FnMut(DeviceStart)) -> Result<(), String> {
    let http = reqwest::Client::new();
    let name = format!("Parameters app ({})", std::env::consts::OS);
    let (status, body) = post(&http, "/auth/device", &json!({ "name": name })).await?;
    if status != 200 {
        return Err(message(&body));
    }
    let start: DeviceStart = serde_json::from_value(body).map_err(|error| error.to_string())?;
    let _ = webbrowser::open(&start.verification_uri);
    let claim = json!({ "device_code": start.device_code });
    let interval = Duration::from_secs(start.interval.max(1));
    let deadline = Instant::now() + Duration::from_secs(start.expires_in);
    show(start);
    while Instant::now() < deadline {
        tokio::time::sleep(interval).await;
        match post(&http, "/auth/device/token", &claim).await? {
            (200, body) => {
                let token = body["token"]
                    .as_str()
                    .ok_or("the server did not send a session")?;
                write_private(SESSION_FILE, token).await?;
                authorize(token);
                return Ok(());
            }
            (202, _) => {}
            (_, body) => return Err(message(&body)),
        }
    }
    Err("the sign-in expired; start again".into())
}

pub(crate) async fn sign_out() {
    let headers = dioxus::fullstack::get_request_headers();
    let _ = reqwest::Client::new()
        .post(format!("{}/auth/signout", server()))
        .headers(headers)
        .send()
        .await;
    clear_request_headers();
    if let Some(file) = path(SESSION_FILE) {
        let _ = tokio::fs::remove_file(file).await;
    }
}
