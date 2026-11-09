//! `app-edge` — the single public entry point.
//!
//! - Forwards requests to the service that owns the path (longest prefix from `services.json`)
//!   through the bridges (`Ctx::call`): in this process or over HTTP, streaming bodies both ways.
//! - `GET /api/health`: health of every service, whether it is bridged, and the app name.
//! - Everything else: the built SPA from `WEB_DIST` (`web/dist`) with history fallback to
//!   `index.html`. In development Vite serves the SPA and proxies `/api` here.

mod health;
mod proxy;
mod spa;
#[cfg(test)]
mod tests;

use app_core::Ctx;
use axum::{Router, routing::get};

/// The edge: its routes. Runs alone (`main.rs`) or with the other services in `app-all`.
///
/// # Errors
/// Never yet; a service that needs a startup step returns its error here.
pub async fn build(ctx: Ctx) -> anyhow::Result<Router> {
    Ok(router(ctx))
}

fn router(ctx: Ctx) -> Router {
    Router::new()
        .route("/api/health", get(health::health_all))
        .fallback(proxy::handle)
        .with_state(ctx)
}
