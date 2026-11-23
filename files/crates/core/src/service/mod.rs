//! Service bootstrap: every service is a library with `build(ctx)` (its routes and background
//! work), and its `services/<name>/src/main.rs` is one call:
//!
//! ```ignore
//! fn main() -> anyhow::Result<()> {
//!     app_core::service::main("notes", app_notes::build)
//! }
//! ```
//!
//! [`main`] / [`run`] load the config, set up tracing, open the database (applying migrations),
//! build a [`Ctx`], add `GET /health`, request tracing, the request body limit (`MAX_BODY_BYTES`)
//! and graceful shutdown, and listen on the port of the service from `services.json`. Servers pass
//! the TCP peer address to handlers (`ConnectInfo<SocketAddr>`): the edge records it as the client
//! address (`crate::client_ip`).
//!
//! `app-all` calls [`host`] instead: every service in one process, over one database pool and one
//! HTTP client, connected by [`bridges`](crate::bridge) (`docs/bridges.md`).

mod ctx;

use std::{collections::HashMap, future::Future, net::SocketAddr, pin::Pin, time::Duration};

use anyhow::{Context as _, bail};
use axum::{
    Router,
    body::Body,
    extract::{DefaultBodyLimit, connect_info::IntoMakeServiceWithConnectInfo},
    http,
    routing::get,
};
pub use ctx::{Ctx, CtxInner};
use serde_json::json;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{config::Config, error::Json};

/// How long in-flight requests may finish after a stop signal.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Initializes tracing (`LOG_LEVEL` / `RUST_LOG`) once per process. `with_target` adds the module
/// to every line, which tells the services apart when they share a process.
pub fn init_tracing(config: &Config, with_target: bool) {
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    // Fails when a subscriber is already installed (a test, another service in this process).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(with_target)
        .compact()
        .try_init();
}

/// The `build` function of a service, as `app-all` lists it.
pub type Build = fn(Ctx) -> Pin<Box<dyn Future<Output = anyhow::Result<Router>> + Send>>;

/// The whole `main` of a separate service binary: loads the config, starts a Tokio runtime with
/// [`Config::worker_threads`] workers and [`run`]s service `name`.
///
/// # Errors
/// When startup fails (config, database, port in use) or the server stops with an error.
pub fn main<F, Fut>(name: &str, build: F) -> anyhow::Result<()>
where
    F: FnOnce(Ctx) -> Fut,
    Fut: Future<Output = anyhow::Result<Router>>,
{
    let config = Config::load()?;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(config.worker_threads())
        .enable_all()
        .build()?
        .block_on(serve_one(config, name, build))
}

/// Runs service `name` in this process alone: see the module docs. `build` gets the [`Ctx`] and
/// returns the service's routes; it may also spawn background tasks (`tokio::spawn`).
///
/// # Errors
/// When startup fails (config, database, port in use) or the server stops with an error.
pub async fn run<F, Fut>(name: &str, build: F) -> anyhow::Result<()>
where
    F: FnOnce(Ctx) -> Fut,
    Fut: Future<Output = anyhow::Result<Router>>,
{
    serve_one(Config::load()?, name, build).await
}

async fn serve_one<F, Fut>(config: Config, name: &str, build: F) -> anyhow::Result<()>
where
    F: FnOnce(Ctx) -> Fut,
    Fut: Future<Output = anyhow::Result<Router>>,
{
    init_tracing(&config, false);
    let addr = address(&config, name);
    let ctx = Ctx::new(name, config).await?;
    let router = finish(name, &ctx.config, build(ctx.clone()).await?);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("{name}: cannot listen on {addr}"))?;
    tracing::info!("{name} listening on http://{addr}");
    axum::serve(listener, with_peer(router))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    tracing::info!("{name} stopped");
    Ok(())
}

/// Runs `services` in this process: see the module docs. Only the edge listens, unless some service
/// runs separately (`Config::separate_services`): then every hosted service listens too, so the
/// separate ones can reach it on its own port.
///
/// # Errors
/// When startup fails (config, database, port in use) or a server stops with an error.
pub async fn host(config: Config, services: &[(&str, Build)]) -> anyhow::Result<()> {
    init_tracing(&config, true);
    let separate = config.separate_services();
    let hosted: Vec<(&str, Build)> = services
        .iter()
        .filter(|(name, _)| !separate.contains(name))
        .copied()
        .collect();
    if hosted.is_empty() {
        bail!("nothing to host with BRIDGES=off: run `node scripts/start.ts` or set BRIDGES=on");
    }
    let public: Vec<&str> = if separate.is_empty() {
        vec!["edge"]
    } else {
        hosted.iter().map(|(name, _)| *name).collect()
    };
    // Ports first: a second copy of the app stops here, before it starts any background work.
    let mut listeners = Vec::with_capacity(public.len());
    for name in public {
        let addr = address(&config, name);
        let listener = tokio::net::TcpListener::bind(&addr)
            .await
            .with_context(|| format!("{name}: cannot listen on {addr}"))?;
        listeners.push((name, addr, listener));
    }
    let separate: Vec<String> = separate.into_iter().map(str::to_owned).collect();

    let ctx = Ctx::new("app", config).await?;
    let routers = assemble(&ctx, &hosted).await?;
    let mut servers = Vec::with_capacity(listeners.len());
    for (name, addr, listener) in listeners {
        let router = routers
            .get(name)
            .with_context(|| format!("{name} is not linked into this binary"))?
            .clone();
        tracing::info!("{name} listening on http://{addr}");
        servers.push(async move {
            axum::serve(listener, with_peer(router))
                .with_graceful_shutdown(shutdown_signal())
                .await
        });
    }
    tracing::info!(hosted = hosted.len(), ?separate, "app is ready");
    tokio::select! {
        result = futures::future::try_join_all(servers) => { result?; }
        () = async {
            shutdown_signal().await;
            tokio::time::sleep(SHUTDOWN_GRACE).await;
        } => tracing::warn!("requests still running after the grace period were dropped"),
    }
    tracing::info!("app stopped");
    Ok(())
}

/// Builds `services` over the resources of `ctx` and connects them with bridges, so that from now
/// on calls between them stay in this process. Returns their routers.
///
/// # Errors
/// When a service cannot start.
pub async fn assemble(
    ctx: &Ctx,
    services: &[(&str, Build)],
) -> anyhow::Result<HashMap<String, Router>> {
    let mut routers = HashMap::with_capacity(services.len());
    for (name, build) in services {
        let router = build(ctx.for_service(name))
            .await
            .with_context(|| format!("starting {name}"))?;
        routers.insert((*name).to_owned(), finish(name, &ctx.config, router));
    }
    ctx.bridges.connect(routers.clone());
    Ok(routers)
}

fn address(config: &Config, name: &str) -> String {
    format!("{}:{}", config.bind_host, config.port(name))
}

/// The router as a server that records the peer address of every connection.
fn with_peer(router: Router) -> IntoMakeServiceWithConnectInfo<Router, SocketAddr> {
    router.into_make_service_with_connect_info::<SocketAddr>()
}

/// Adds `GET /health`, request tracing and the body limit: extractors (`Json`, `Bytes`, `Form`)
/// reject bodies over `MAX_BODY_BYTES` with 413. The span names the service, so requests of
/// services sharing a process stay apart in the logs (a bridged call is a span inside the
/// caller's one).
fn finish(name: &str, config: &Config, router: Router) -> Router {
    let service = name.to_owned();
    let trace = TraceLayer::new_for_http().make_span_with(move |req: &http::Request<Body>| {
        tracing::debug_span!("request", service = %service, method = %req.method(), uri = %req.uri())
    });
    router
        .merge(
            Router::new()
                .route("/health", get(health))
                .with_state(name.to_owned()),
        )
        .layer(DefaultBodyLimit::max(config.max_body_bytes))
        .layer(trace)
}

async fn health(
    axum::extract::State(name): axum::extract::State<String>,
) -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "service": name, "version": env!("CARGO_PKG_VERSION") }))
}

/// Resolves on Ctrl+C or SIGTERM.
pub async fn shutdown_signal() {
    let ctrl_c = async {
        // An error (no handler could be installed) ends the wait too, so the service still stops.
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {},
        () = terminate => {},
    }
}
