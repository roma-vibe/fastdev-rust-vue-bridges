//! A user's journey through the edge (accounts on) and the browser-like client it uses.

use std::collections::HashMap;

use app_core::{Config, crypto};
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use tower::ServiceExt as _;

/// A client of the edge that keeps the session cookie and sends `Origin` like a browser on the
/// app's page (the default `PUBLIC_URL`), so cookie-signed writes pass the origin check.
pub(super) struct Client {
    app: Router,
    cookie: Option<String>,
    origin: String,
}

impl Client {
    pub(super) fn new(app: &Router) -> Self {
        let vars = HashMap::from([("MASTER_KEY".to_owned(), crypto::generate_master_key())]);
        let config = Config::from_vars(std::env::temp_dir(), vars).unwrap();
        Self {
            app: app.clone(),
            cookie: None,
            origin: app_core::config::origin_of(&config.public_url).unwrap(),
        }
    }

    pub(super) async fn send(
        &mut self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<Value>,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::ORIGIN, &self.origin);
        if let Some(cookie) = &self.cookie {
            req = req.header(header::COOKIE, cookie);
        }
        if let Some(token) = bearer {
            req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        let req = match body {
            Some(body) => req
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string())),
            None => req.body(Body::empty()),
        };
        let response = self.app.clone().oneshot(req.unwrap()).await.unwrap();
        if let Some(set) = response.headers().get(header::SET_COOKIE) {
            let pair = set.to_str().unwrap().split(';').next().unwrap().to_owned();
            self.cookie = (!pair.ends_with('=')).then_some(pair);
        }
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, value)
    }
}

/// What a user does with every service through the edge: an account (identity), a session cookie,
/// a personal API key, notes over that key (notes authenticates against identity's tables), the
/// admin list, errors, logout.
pub(super) async fn check_journey(app: &Router) {
    let mut browser = Client::new(app);
    let (status, user) = browser
        .send(
            "POST",
            "/api/auth/register",
            None,
            Some(json!({"email": "a@example.com", "password": "correct horse", "name": "A"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{user}");
    assert!(browser.cookie.is_some(), "the cookie passes the edge");

    let (status, me) = browser.send("GET", "/api/me", None, None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(me["email"], "a@example.com");
    assert_eq!(me["role"], "admin");

    let (status, key) = browser
        .send("POST", "/api/keys", None, Some(json!({"name": "script"})))
        .await;
    assert_eq!(status, StatusCode::CREATED, "{key}");
    let secret = key["secret"].as_str().unwrap().to_owned();

    let mut script = Client::new(app);
    let (status, note) = script
        .send(
            "POST",
            "/api/notes",
            Some(&secret),
            Some(json!({"title": "Hello", "body": "over the bridge"})),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{note}");
    let id = note["id"].as_str().unwrap();
    let (status, note) = script
        .send(
            "PATCH",
            &format!("/api/notes/{id}?x=1"),
            Some(&secret),
            Some(json!({"pinned": true})),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{note}");
    assert_eq!(note["pinned"], true);
    let (status, list) = browser.send("GET", "/api/notes", None, None).await;
    assert_eq!(status, StatusCode::OK, "{list}");
    assert_eq!(list["items"].as_array().unwrap().len(), 1);

    let (status, users) = browser.send("GET", "/api/admin/users", None, None).await;
    assert_eq!(status, StatusCode::OK, "{users}");

    let page = std::mem::replace(&mut browser.origin, "https://evil.example".to_owned());
    let (status, error) = browser
        .send("POST", "/api/notes", None, Some(json!({"title": "CSRF"})))
        .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "a foreign page: {error}");
    browser.origin = page;

    let (status, error) = script
        .send(
            "POST",
            "/api/notes",
            Some(&secret),
            Some(json!({"title": ""})),
        )
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{error}");
    assert_eq!(error["error"]["code"], "bad_request");
    let (status, _) = Client::new(app).send("GET", "/api/notes", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = browser.send("GET", "/api/nothing", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = browser.send("POST", "/api/auth/logout", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(
        browser.cookie.is_none(),
        "the cleared cookie passes the edge"
    );
    let (status, _) = browser.send("GET", "/api/me", None, None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
