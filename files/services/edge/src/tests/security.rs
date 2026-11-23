//! Security headers on every kind of response and the client address passed to the services.

use std::net::SocketAddr;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request},
};
use tower::ServiceExt as _;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{header, method, path},
};

use super::{edge, edge_with};
use crate::headers::{API_CSP, APP_CSP};

fn assert_always(headers: &HeaderMap) {
    assert_eq!(headers["x-content-type-options"], "nosniff");
    assert_eq!(
        headers["referrer-policy"],
        "strict-origin-when-cross-origin"
    );
    assert_eq!(headers["x-frame-options"], "DENY");
    assert_eq!(headers["cross-origin-opener-policy"], "same-origin");
    assert_eq!(
        headers["permissions-policy"],
        "camera=(), microphone=(), geolocation=()"
    );
}

#[tokio::test]
async fn api_responses_get_the_strict_policy() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/notes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "items": [] })))
        .mount(&mock)
        .await;
    let app = edge(&mock, "/nonexistent").await;
    for uri in ["/api/notes", "/api/unknown", "/api/health"] {
        let response = app
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let headers = response.headers();
        assert_always(headers);
        assert_eq!(headers["content-security-policy"], API_CSP, "{uri}");
        assert!(
            !headers.contains_key("strict-transport-security"),
            "plain http"
        );
        assert!(!headers.contains_key("cache-control"), "anonymous");
    }
}

#[tokio::test]
async fn the_built_app_gets_the_page_policy_and_https_gets_hsts() {
    let mock = MockServer::start().await;
    let dist = std::env::temp_dir().join(format!("app-edge-csp-{}", app_core::new_id()));
    std::fs::create_dir_all(&dist).unwrap();
    std::fs::write(dist.join("index.html"), "<html>spa</html>").unwrap();
    std::fs::write(dist.join("app.js"), "export {}").unwrap();
    let app = edge_with(
        &mock.uri(),
        dist.to_str().unwrap(),
        &[("PUBLIC_URL", "https://app.example.com")],
    )
    .await;
    let page = app
        .clone()
        .oneshot(Request::get("/notes").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_always(page.headers());
    assert_eq!(page.headers()["content-security-policy"], APP_CSP);
    assert_eq!(
        page.headers()["strict-transport-security"],
        "max-age=31536000"
    );
    let script = app
        .oneshot(Request::get("/app.js").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(script.headers()["content-security-policy"], API_CSP);
    std::fs::remove_dir_all(dist).unwrap();
}

#[tokio::test]
async fn signed_in_responses_are_not_stored() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/notes"))
        .respond_with(ResponseTemplate::new(200).insert_header("cache-control", "max-age=60"))
        .mount(&mock)
        .await;
    let response = edge(&mock, "/nonexistent")
        .await
        .oneshot(
            Request::get("/api/notes")
                .header("authorization", "Bearer sk-test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
}

/// Sends `GET /api/notes` from `peer` with `headers`; the mock answers 200 only when the service
/// receives `x-client-ip: expected`.
async fn client_ip_seen(trust_proxy: &str, headers: &[(&str, &str)], expected: &str) -> u16 {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/notes"))
        .and(header("x-client-ip", expected))
        .respond_with(ResponseTemplate::new(200))
        .mount(&mock)
        .await;
    let app = edge_with(&mock.uri(), "/nonexistent", &[("TRUST_PROXY", trust_proxy)]).await;
    let mut request = Request::get("/api/notes");
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut request = request.body(Body::empty()).unwrap();
    let peer: SocketAddr = "198.51.100.4:5555".parse().unwrap();
    request.extensions_mut().insert(ConnectInfo(peer));
    app.oneshot(request).await.unwrap().status().as_u16()
}

#[tokio::test]
async fn services_get_the_client_address_from_the_edge_only() {
    let spoofed = [
        ("x-client-ip", "10.9.9.9"),
        ("x-forwarded-for", "6.6.6.6, 203.0.113.7"),
    ];
    assert_eq!(client_ip_seen("off", &spoofed, "198.51.100.4").await, 200);
    assert_eq!(client_ip_seen("on", &spoofed, "203.0.113.7").await, 200);
    assert_eq!(client_ip_seen("on", &[], "198.51.100.4").await, 200);
}

#[tokio::test]
async fn a_foreign_host_gets_421_and_known_hosts_pass() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/notes"))
        .respond_with(ResponseTemplate::new(201))
        .mount(&mock)
        .await;
    let app = edge_with(
        &mock.uri(),
        "/nonexistent",
        &[("PUBLIC_URL", "https://app.example.com")],
    )
    .await;
    let send = |host: &'static str| {
        let app = app.clone();
        async move {
            app.oneshot(
                Request::post("/api/notes")
                    .header("host", host)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
        }
    };
    let refused = send("rebind.attacker.example:8700").await;
    assert_eq!(refused.status(), 421);
    assert_always(refused.headers());
    assert_eq!(send("localhost:8700").await.status(), 201);
    assert_eq!(send("app.example.com").await.status(), 201);
}
