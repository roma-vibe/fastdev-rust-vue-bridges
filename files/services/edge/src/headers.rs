//! Security headers on every response of the edge, the only server browsers talk to, so they hold
//! in one process and with separate services alike. In development Vite serves the page itself
//! (only `/api` passes the edge); these headers then reach the browser on API responses only.
//!
//! - always: `nosniff`, a strict `Referrer-Policy`, no framing, an isolated opener and no
//!   camera, microphone or geolocation;
//! - `Strict-Transport-Security` when `PUBLIC_URL` is https;
//! - HTML (the built app from `web/dist`): [`APP_CSP`]; everything else (JSON, assets): [`API_CSP`];
//! - a request with a session cookie or a key, and a response that sets a cookie, get
//!   `Cache-Control: no-store`.

use app_core::{Ctx, caller::carries_credentials};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

/// CSP of the built web app. Vite emits only external scripts and stylesheets (no inline code),
/// and Vue sets `:style` through the CSSOM, which `style-src` does not restrict. Two relaxations
/// are needed: `img-src data:` (the backdrop's grain is an inline SVG `background-image`) and
/// `font-src data:` (Vite inlines assets under 4 KiB, e.g. a small font subset, as `data:` URLs).
pub const APP_CSP: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data:; connect-src 'self'; font-src 'self' data:; object-src 'none'; \
     base-uri 'self'; form-action 'self'; frame-ancestors 'none'";

/// CSP of everything that is not a page (JSON, files): nothing may load or frame it.
pub const API_CSP: &str = "default-src 'none'; frame-ancestors 'none'";

const ALWAYS: [(&str, &str); 5] = [
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "strict-origin-when-cross-origin"),
    ("x-frame-options", "DENY"),
    ("cross-origin-opener-policy", "same-origin"),
    (
        "permissions-policy",
        "camera=(), microphone=(), geolocation=()",
    ),
];

/// Middleware: adds the headers to the response of every request.
pub async fn secure(State(ctx): State<Ctx>, req: Request, next: Next) -> Response {
    let signed_in = carries_credentials(req.headers(), &ctx.config);
    let https = ctx.config.public_url.starts_with("https://");
    let mut response = next.run(req).await;
    apply(response.headers_mut(), https, signed_in);
    response
}

fn apply(headers: &mut HeaderMap, https: bool, signed_in: bool) {
    for (name, value) in ALWAYS {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    if https {
        headers.insert(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static("max-age=31536000"),
        );
    }
    let html = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("text/html"));
    let csp = if html { APP_CSP } else { API_CSP };
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(csp),
    );
    if signed_in || headers.contains_key(header::SET_COOKIE) {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_and_api_get_their_own_policy() {
        let mut page = HeaderMap::new();
        page.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        apply(&mut page, false, false);
        assert_eq!(page[header::CONTENT_SECURITY_POLICY], APP_CSP);
        assert!(!page.contains_key(header::STRICT_TRANSPORT_SECURITY));
        assert!(!page.contains_key(header::CACHE_CONTROL));

        let mut api = HeaderMap::new();
        api.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        apply(&mut api, true, true);
        assert_eq!(api[header::CONTENT_SECURITY_POLICY], API_CSP);
        assert_eq!(api[header::STRICT_TRANSPORT_SECURITY], "max-age=31536000");
        assert_eq!(api[header::CACHE_CONTROL], "no-store");
        assert_eq!(api["x-frame-options"], "DENY");

        let mut sign_in = HeaderMap::new();
        sign_in.insert(header::SET_COOKIE, HeaderValue::from_static("s=1"));
        apply(&mut sign_in, false, false);
        assert_eq!(sign_in[header::CACHE_CONTROL], "no-store");
    }
}
