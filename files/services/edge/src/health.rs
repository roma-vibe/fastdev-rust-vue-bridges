//! `GET /api/health`:
//! `{status: "ok"|"degraded", appName, services: [{name, ok, bridged, latencyMs, version}]}`.
//! `bridged` tells whether the service runs in this process (`docs/bridges.md`).

use std::time::{Duration, Instant};

use app_core::{Ctx, Json, registry};
use axum::{
    body::{Body, to_bytes},
    extract::{Request, State},
};
use serde::Serialize;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceHealth {
    name: String,
    ok: bool,
    bridged: bool,
    latency_ms: u64,
    version: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthAll {
    status: &'static str,
    app_name: String,
    services: Vec<ServiceHealth>,
}

pub async fn health_all(State(ctx): State<Ctx>) -> Json<HealthAll> {
    let checks = registry::services()
        .iter()
        .filter(|s| s.name != "edge")
        .map(|s| check(&ctx, &s.name));
    let mut services = futures::future::join_all(checks).await;
    services.sort_by(|a, b| a.name.cmp(&b.name));
    let status = if services.iter().all(|s| s.ok) {
        "ok"
    } else {
        "degraded"
    };
    Json(HealthAll {
        status,
        app_name: ctx.config.app_name.clone(),
        services,
    })
}

async fn check(ctx: &Ctx, name: &str) -> ServiceHealth {
    let started = Instant::now();
    let version = tokio::time::timeout(Duration::from_secs(2), version_of(ctx, name))
        .await
        .ok()
        .flatten();
    let latency_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    ServiceHealth {
        name: name.to_owned(),
        ok: version.is_some(),
        bridged: ctx.bridges.is_local(name),
        latency_ms,
        version: version.flatten(),
    }
}

/// `Some(version)` when the service answers its `/health`.
async fn version_of(ctx: &Ctx, name: &str) -> Option<Option<String>> {
    let req = Request::get("/health").body(Body::empty()).ok()?;
    let response = ctx.call(name, req).await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.ok()?;
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap_or_default();
    Some(
        body.get("version")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    )
}
