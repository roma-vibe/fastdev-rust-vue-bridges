//! Host check against DNS rebinding: a page on an attacker's domain that re-points its name at
//! 127.0.0.1 makes the browser send requests to this edge with `Host: attacker.example`. Binding to
//! loopback does not stop that; refusing unknown hosts does.
//!
//! Allowed: `localhost`, `127.0.0.1` and `[::1]` on any port, and the host of every allowed origin
//! (`PUBLIC_URL`, the Vite dev server, `ALLOWED_ORIGINS`). A published app's hostname therefore
//! goes into `PUBLIC_URL` (or `ALLOWED_ORIGINS`). Anything else gets 421. A request without a host
//! (tests, raw HTTP/1.0 tools) passes: browsers always send one.

use app_core::Ctx;
use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

const LOOPBACK: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// Middleware: answers 421 to a request whose host is not allowed.
pub async fn check(State(ctx): State<Ctx>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_owned()));
    match host {
        Some(host) if !allowed(&host, &ctx.config.allowed_origins) => misdirected(),
        _ => next.run(req).await,
    }
}

fn misdirected() -> Response {
    let body = json!({ "error": {
        "code": "misdirected",
        "message": "This server does not answer for that host name.",
    }});
    (StatusCode::MISDIRECTED_REQUEST, Json(body)).into_response()
}

/// Whether `host` (a `Host` header value, `name[:port]`) is loopback or the host of an origin.
fn allowed(host: &str, origins: &[String]) -> bool {
    let Some(name) = name_of(host) else {
        return false;
    };
    LOOPBACK.contains(&name.as_str())
        || origins
            .iter()
            .filter_map(|origin| origin.split_once("://").and_then(|(_, rest)| name_of(rest)))
            .any(|allowed| allowed == name)
}

/// The lowercase host name of `name[:port]` without the port or a trailing dot; IPv6 keeps its
/// brackets. `None` for an empty or malformed value.
fn name_of(host: &str) -> Option<String> {
    let host = host.trim().to_ascii_lowercase();
    let name = if host.starts_with('[') {
        let end = host.find(']')?;
        let rest = &host[end + 1..];
        if !(rest.is_empty() || rest.starts_with(':')) {
            return None;
        }
        host[..=end].to_owned()
    } else {
        match host.rsplit_once(':') {
            Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name.to_owned(),
            Some(_) => return None,
            None => host,
        }
    };
    let name = name.trim_end_matches('.').to_owned();
    (!name.is_empty() && !name.contains(['/', '@', ' '])).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origins() -> Vec<String> {
        vec![
            "https://app.example.com".to_owned(),
            "http://localhost:5173".to_owned(),
            "http://[::1]:5173".to_owned(),
        ]
    }

    #[test]
    fn loopback_and_origin_hosts_pass_on_any_port() {
        for host in [
            "localhost",
            "localhost:8700",
            "127.0.0.1:5173",
            "[::1]",
            "[::1]:8700",
            "app.example.com",
            "APP.Example.com.",
            "app.example.com:443",
        ] {
            assert!(allowed(host, &origins()), "{host} should pass");
        }
    }

    #[test]
    fn other_hosts_are_refused() {
        for host in [
            "rebind.attacker.example:8700",
            "example.com",
            "app.example.com.attacker.example",
            "localhost.attacker.example",
            "127.0.0.2",
            "0.0.0.0:8700",
            "[::2]",
            "[::1]x",
            "user@localhost",
            "localhost:abc",
            "",
        ] {
            assert!(!allowed(host, &origins()), "{host:?} should be refused");
        }
    }
}
