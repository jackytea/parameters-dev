use std::path::PathBuf;
use std::process::ExitCode;

use dioxus::fullstack::FullstackContext;
use dioxus::prelude::*;
use dioxus::server::axum::extract::Request;
use dioxus::server::axum::middleware::{self, Next};
use dioxus::server::axum::response::{IntoResponse, Response};
use dioxus::server::axum::{self, Extension};
use dioxus::server::http::uri::Authority;
use dioxus::server::http::{StatusCode, header};
use parameters_api::{ApiError, Via};
use parameters_service::{Actor, Config, Service};
use tokio_util::sync::CancellationToken;

use crate::failure::Failure;

const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

pub(crate) fn main() -> ExitCode {
    dioxus::logger::initialize_default();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("error: cannot start the runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(serve()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> Result<(), String> {
    let config = Config::from_env(dioxus::cli_config::fullstack_address_or_localhost())
        .map_err(|error| error.to_string())?;
    let (service, background) = Service::start(&config)
        .await
        .map_err(|error| error.to_string())?;
    let router = if has_client_bundle().await {
        axum::Router::new().serve_dioxus_application(ServeConfig::new(), crate::App)
    } else {
        tracing::warn!("no client bundle next to the server, so only the API is served");
        axum::Router::new().serve_api_application(ServeConfig::new(), crate::App)
    };
    let mcp = CancellationToken::new();
    let router = router
        .nest_service(
            "/mcp",
            parameters_mcp::http_service(service.clone(), mcp.clone()),
        )
        .layer(Extension(service))
        .layer(middleware::from_fn(local_only));
    let served = match tokio::net::TcpListener::bind(config.address).await {
        Ok(listener) => {
            tracing::info!("Parameters is listening on http://{}", config.address);
            axum::serve(listener, router)
                .with_graceful_shutdown(stopped())
                .await
                .map_err(|error| error.to_string())
        }
        Err(error) => Err(format!("cannot listen on {}: {error}", config.address)),
    };
    mcp.cancel();
    background.shutdown().await;
    served
}

async fn has_client_bundle() -> bool {
    let directory = match std::env::var_os("DIOXUS_PUBLIC_PATH") {
        Some(path) => PathBuf::from(path),
        None => match std::env::current_exe() {
            Ok(executable) => executable.with_file_name("public"),
            Err(_) => return false,
        },
    };
    tokio::fs::metadata(directory)
        .await
        .is_ok_and(|metadata| metadata.is_dir())
}

async fn stopped() {
    #[cfg(unix)]
    if let Ok(mut terminate) =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}

async fn local_only(request: Request, next: Next) -> Response {
    if is_local(&request) {
        next.run(request).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "Parameters only accepts requests addressed to localhost",
        )
            .into_response()
    }
}

fn is_local(request: &Request) -> bool {
    let headers = request.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .or_else(|| request.uri().authority().map(Authority::as_str));
    if !host.is_some_and(is_local_authority) {
        return false;
    }
    headers.get(header::ORIGIN).is_none_or(|origin| {
        origin
            .to_str()
            .ok()
            .and_then(|origin| origin.split_once("://"))
            .is_some_and(|(scheme, authority)| {
                matches!(scheme, "http" | "https") && is_local_authority(authority)
            })
    })
}

fn is_local_authority(authority: &str) -> bool {
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((host, port)) => (host, port),
            None => return false,
        },
        None => match authority.split_once(':') {
            Some((host, port)) => (host, authority.get(host.len()..).unwrap_or(port)),
            None => (authority, ""),
        },
    };
    let port_ok = port.is_empty()
        || port.strip_prefix(':').is_some_and(|digits| {
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        });
    port_ok
        && LOCAL_HOSTS
            .iter()
            .any(|local| local.eq_ignore_ascii_case(host))
}

pub(crate) fn service() -> Result<Service, Failure> {
    FullstackContext::current()
        .and_then(|context| context.extension::<Service>())
        .ok_or_else(|| Failure(ApiError::internal()))
}

pub(crate) const fn ui() -> Actor {
    Actor::local(Via::Ui)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(host: Option<&str>, origin: Option<&str>) -> Request {
        let mut builder = Request::builder().uri("/api/workflows");
        if let Some(host) = host {
            builder = builder.header(header::HOST, host);
        }
        if let Some(origin) = origin {
            builder = builder.header(header::ORIGIN, origin);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn accepts_only_local_hosts_and_origins() {
        for host in [
            "localhost",
            "localhost:8080",
            "127.0.0.1:1",
            "[::1]:8080",
            "LOCALHOST",
        ] {
            assert!(is_local(&request(Some(host), None)), "{host}");
        }
        for host in [
            "evil.example",
            "localhost.evil.example",
            "127.0.0.2",
            "[::2]:80",
            "localhost:",
            "localhost:80x",
            "[::1",
        ] {
            assert!(!is_local(&request(Some(host), None)), "{host}");
        }
        assert!(!is_local(&request(None, None)));
        assert!(is_local(&request(
            Some("localhost:8080"),
            Some("http://localhost:8080")
        )));
        assert!(is_local(&request(
            Some("localhost:8080"),
            Some("https://127.0.0.1")
        )));
        for origin in [
            "http://evil.example",
            "null",
            "file://",
            "http://localhost.evil.example",
        ] {
            assert!(
                !is_local(&request(Some("localhost:8080"), Some(origin))),
                "{origin}"
            );
        }
    }
}
