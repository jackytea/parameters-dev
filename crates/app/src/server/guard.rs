use dioxus::server::axum::extract::Request;
use dioxus::server::axum::middleware::Next;
use dioxus::server::axum::response::{IntoResponse, Response};
use dioxus::server::http::uri::Authority;
use dioxus::server::http::{StatusCode, header};

const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "::1"];

pub(crate) async fn local_only(request: Request, next: Next) -> Response {
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
        builder
            .body(dioxus::server::axum::body::Body::empty())
            .unwrap()
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
