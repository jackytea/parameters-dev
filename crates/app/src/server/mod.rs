mod auth;
mod guard;
mod headers;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use dioxus::fullstack::FullstackContext;
use dioxus::prelude::*;
use dioxus::server::axum::{self, Extension, middleware};
use parameters_api::ApiError;
use parameters_service::{Actor, Config, Service};
use tokio_util::sync::CancellationToken;

use crate::failure::Failure;

pub(crate) use auth::ClientAddress;

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
    let mut router = router
        .merge(auth::routes(service.clone()))
        .nest_service(
            "/mcp",
            parameters_mcp::http_service(service.clone(), mcp.clone()),
        )
        .layer(middleware::from_fn_with_state(
            service.clone(),
            auth::authenticate,
        ))
        .layer(Extension(service.clone()));
    if !service.is_managed() {
        router = router.layer(middleware::from_fn(guard::local_only));
    }
    let router = router.layer(middleware::from_fn_with_state(
        service.public_url().map(str::to_owned),
        headers::secure,
    ));
    let served = match tokio::net::TcpListener::bind(config.address).await {
        Ok(listener) => {
            tracing::info!("Parameters is listening on http://{}", config.address);
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
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

pub(crate) fn service() -> Result<Service, Failure> {
    FullstackContext::current()
        .and_then(|context| context.extension::<Service>())
        .ok_or_else(|| Failure(ApiError::internal()))
}

pub(crate) fn signed_in() -> Option<Actor> {
    FullstackContext::current().and_then(|context| context.extension::<Actor>())
}

pub(crate) fn actor() -> Result<Actor, Failure> {
    signed_in().ok_or_else(|| Failure(ApiError::signed_out()))
}

pub(crate) fn client_address() -> String {
    FullstackContext::current()
        .and_then(|context| context.extension::<ClientAddress>())
        .map(|address| address.0)
        .unwrap_or_default()
}
