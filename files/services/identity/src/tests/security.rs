//! The origin check, sessions per sign-in and sign-in throttling.

use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};

use super::{Browser, ctx};

#[tokio::test]
async fn cookie_writes_need_an_allowed_origin() {
    let ctx = ctx().await;
    let mut ann = Browser::new(&ctx);
    ann.register("ann@example.com").await;
    let page = ann.origin.clone();
    let rename = || Some(json!({ "name": "Mallory" }));

    for origin in [Some("https://evil.example".to_owned()), None] {
        ann.origin = origin;
        let (status, body) = ann.send(Method::PATCH, "/api/me", rename()).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
        assert_eq!(body["error"]["code"], "forbidden");
        let (status, _) = ann.send(Method::POST, "/api/auth/logout", None).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let (status, _) = ann.send(Method::GET, "/api/me", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "reads pass and the session survived"
    );

    let token = ann.cookie.clone().unwrap();
    ann.bearer = Some(token.split_once('=').unwrap().1.to_owned());
    let (status, _) = ann.send(Method::PATCH, "/api/me", rename()).await;
    assert_eq!(status, StatusCode::OK, "bearer requests are exempt");

    ann.bearer = None;
    ann.origin = page;
    let (status, _) = ann.send(Method::POST, "/api/auth/logout", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn every_sign_in_gets_its_own_session() {
    let ctx = ctx().await;
    Browser::new(&ctx).register("ann@example.com").await;
    let mut laptop = Browser::new(&ctx);
    let mut phone = laptop.sibling("192.0.2.2");
    assert_eq!(
        laptop.login("ann@example.com", "password123").await.0,
        StatusCode::OK
    );
    assert_eq!(
        phone.login("ann@example.com", "password123").await.0,
        StatusCode::OK
    );
    assert_ne!(laptop.cookie, phone.cookie, "a fresh token per sign-in");

    let (status, _) = laptop.send(Method::POST, "/api/auth/logout", None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(
        phone.send(Method::GET, "/api/me", None).await.0,
        StatusCode::OK
    );
}

/// Five wrong passwords, then the sixth attempt (status, body).
async fn five_failures(browser: &mut Browser, email: &str) -> Vec<(StatusCode, Value)> {
    let mut answers = Vec::new();
    for _ in 0..6 {
        answers.push(browser.login(email, "wrong-password").await);
    }
    answers
}

#[tokio::test]
async fn failures_slow_down_the_email_and_the_client_alike_for_unknown_emails() {
    let ctx = ctx().await;
    let mut owner = Browser::new(&ctx);
    owner.register("ann@example.com").await;

    let known = five_failures(&mut owner.sibling("203.0.113.1"), "ann@example.com").await;
    let mut elsewhere = owner.sibling("198.51.100.9");
    let (status, _) = elsewhere.login("ann@example.com", "password123").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "the email waits too");

    let unknown = five_failures(&mut owner.sibling("203.0.113.2"), "nobody@example.com").await;
    assert_eq!(known, unknown, "nothing tells whether an email exists");
    assert!(
        known[..5]
            .iter()
            .all(|(s, _)| *s == StatusCode::UNAUTHORIZED)
    );
    let (status, body) = &known[5];
    assert_eq!(*status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["error"]["code"], "rate_limited");
    assert_eq!(body["retryAfter"], "1");

    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (status, _) = elsewhere.login("ann@example.com", "password123").await;
    assert_eq!(status, StatusCode::OK, "only a wait, never a lockout");

    let mut attacker = owner.sibling("203.0.113.1");
    let (status, _) = attacker.login("other@example.com", "wrong-password").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = attacker.login("other@example.com", "wrong-password").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "the client waits");
    assert_eq!(body["retryAfter"], "2", "the wait doubles");
}

#[tokio::test]
async fn a_successful_sign_in_resets_the_failures() {
    let ctx = ctx().await;
    let mut browser = Browser::new(&ctx);
    browser.register("ann@example.com").await;
    let mut browser = browser.sibling("192.0.2.7");
    for _ in 0..2 {
        for _ in 0..4 {
            let (status, _) = browser.login("ann@example.com", "wrong-password").await;
            assert_eq!(status, StatusCode::UNAUTHORIZED);
        }
        let (status, _) = browser.login("ann@example.com", "password123").await;
        assert_eq!(status, StatusCode::OK);
    }
}

#[tokio::test]
async fn each_client_has_a_budget_of_sign_ins_and_registrations() {
    let ctx = ctx().await;
    let mut first = Browser::new(&ctx);
    first.register("ann@example.com").await;

    let mut client = first.sibling("192.0.2.10");
    for _ in 0..crate::throttle::REGISTRATIONS_PER_HOUR {
        let input = json!({ "email": "x@example.com", "password": "short" });
        let (status, _) = client
            .send(Method::POST, "/api/auth/register", Some(input))
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    let input = json!({ "email": "bob@example.com", "password": "password123" });
    let (status, body) = client
        .send(Method::POST, "/api/auth/register", Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert!(body["retryAfter"].as_str().unwrap().parse::<u64>().unwrap() > 3000);
    let (status, _) = first
        .sibling("192.0.2.11")
        .send(Method::POST, "/api/auth/register", Some(input))
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "other clients are not affected"
    );

    for _ in 0..crate::throttle::SIGN_INS_PER_WINDOW {
        let (status, _) = client.login("ann@example.com", "password123").await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = client.login("ann@example.com", "password123").await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}
