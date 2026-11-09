//! `app-notes` — example service: notes of the current user (`/api/notes`).
//!
//! It shows every layer of a service: `routes` (HTTP: extract, validate, respond), `model` (types
//! and validation), `store` (SQL over the `notes` table this service owns).

mod model;
mod routes;
mod store;
#[cfg(test)]
mod tests;

use app_core::Ctx;
use axum::Router;

/// The service: its routes (and background work, started here with `tokio::spawn`). Runs alone
/// (`main.rs`) or with the other services in `app-all`.
///
/// # Errors
/// Never yet; a service that needs a startup step returns its error here.
pub async fn build(ctx: Ctx) -> anyhow::Result<Router> {
    Ok(routes::router(ctx))
}
