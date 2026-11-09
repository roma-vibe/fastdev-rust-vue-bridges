//! `app-identity` — accounts, sessions and personal API keys (`/api/auth`, `/api/me`, `/api/keys`,
//! `/api/admin/users`). Owns the `users`, `sessions` and `api_keys` tables; every other service
//! authenticates requests through `app_core::auth` (`AuthUser`, `AdminUser`, `Caller`).

mod account;
mod admin;
mod keys;
mod me;
mod password;
mod routes;
#[cfg(test)]
mod tests;
mod users;

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
