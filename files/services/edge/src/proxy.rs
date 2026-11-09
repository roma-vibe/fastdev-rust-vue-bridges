//! Reverse proxy: forwards a request to the service that owns its path, through the bridges
//! (`Ctx::call`): into its router when this process hosts it, else over HTTP.

use app_core::{ApiError, Ctx, registry};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Uri, header},
    response::{IntoResponse, Response},
};

use crate::spa;

/// Hop-by-hop headers are never forwarded (RFC 9110 §7.6.1).
const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

fn copy_headers(from: &HeaderMap, to: &mut HeaderMap) {
    for (name, value) in from {
        if HOP_BY_HOP.contains(&name.as_str()) || name == header::HOST {
            continue;
        }
        to.append(name.clone(), value.clone());
    }
}

/// Fallback handler: a service route, a JSON 404 for unknown `/api` paths, else the SPA.
pub async fn handle(State(ctx): State<Ctx>, req: Request) -> Response {
    let path = req.uri().path().to_owned();
    match registry::route(&path) {
        Some(service) => forward(&ctx, &service.name, req).await,
        None if path == "/api" || path.starts_with("/api/") => {
            ApiError::not_found(format!("No service handles {path}")).into_response()
        }
        None => spa::serve(&ctx, req).await,
    }
}

async fn forward(ctx: &Ctx, service: &str, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let mut headers = HeaderMap::new();
    copy_headers(&parts.headers, &mut headers);
    if let Some(host) = parts.headers.get(header::HOST) {
        headers.insert(HeaderName::from_static("x-forwarded-host"), host.clone());
    }
    headers.insert(
        HeaderName::from_static("x-forwarded-proto"),
        HeaderValue::from_static("http"),
    );
    let mut inner = Request::new(body);
    *inner.method_mut() = parts.method;
    *inner.uri_mut() = parts
        .uri
        .path_and_query()
        .map_or_else(|| Uri::from_static("/"), |p| Uri::from(p.clone()));
    *inner.headers_mut() = headers;

    let upstream = match ctx.call(service, inner).await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(%service, error = %err.detail, "service unreachable");
            return ApiError::unavailable(format!("Service {service} is unavailable"))
                .into_response();
        }
    };
    let (parts, body) = upstream.into_parts();
    let mut response = Response::new(body);
    *response.status_mut() = parts.status;
    copy_headers(&parts.headers, response.headers_mut());
    response
}
