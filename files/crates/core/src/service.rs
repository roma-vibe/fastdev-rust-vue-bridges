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
//! build a [`Ctx`], add `GET /health`, request tracing and graceful shutdown, and listen on the port
//! of the service from `services.json`.
//!
//! `app-all` calls [`host`] instead: every service in one process, over one database pool and one
//! HTTP client, connected by [`bridges`](crate::bridge) (`docs/bridges.md`).

use std::{collections::HashMap, future::Future, ops::Deref, pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context as _, bail};
use axum::{Router, body::Body, extract::Request, http, response::Response, routing::get};
use serde_json::json;
use sqlx::SqlitePool;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

use crate::{
    bridge::{Bridges, Unreachable},
    config::Config,
    crypto::Crypto,
    db,
    error::Json,
};

/// How long in-flight requests may finish after a stop signal.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(10);

/// Shared state of a service: config, database pool, secret encryption, an HTTP client for the
/// outside world and the bridges to the other services. Cheap to clone. Use it as the axum state
/// (or derive `FromRef<YourState> for Ctx`).
#[derive(Clone, Debug)]
pub struct Ctx(Arc<CtxInner>);

#[derive(Debug)]
pub struct CtxInner {
    /// Name of the running service (from `services.json`).
    pub service: String,
    pub config: Config,
    pub db: SqlitePool,
    pub crypto: Crypto,
    /// Shared outbound HTTP client for external APIs (connection pooling); per-request timeouts are
    /// set by callers. Other services of this app are called with [`Ctx::call`], never with it.
    pub http: reqwest::Client,
    /// The way to the other services: use [`Ctx::call`].
    pub bridges: Arc<Bridges>,
}

impl Deref for Ctx {
    type Target = CtxInner;
    fn deref(&self) -> &CtxInner {
        &self.0
    }
}

impl Ctx {
    /// Builds a context from a loaded config: opens the database and applies migrations.
    ///
    /// # Errors
    /// When the database or the master key cannot be used.
    pub async fn new(service: &str, config: Config) -> anyhow::Result<Self> {
        let db = db::connect(&config.db_path).await?;
        Self::with_db(service, config, db)
    }

    /// A context over an existing pool (tests use [`db::connect_memory`]).
    ///
    /// # Errors
    /// When the master key is invalid.
    pub fn with_db(service: &str, config: Config, db: SqlitePool) -> anyhow::Result<Self> {
        let crypto = Crypto::new(&config.master_key)?;
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("app/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self(Arc::new(CtxInner {
            service: service.to_owned(),
            config,
            db,
            crypto,
            http,
            bridges: Arc::default(),
        })))
    }

    /// The same resources (database pool, HTTP client, bridges) under the name of another service
    /// hosted by this process.
    #[must_use]
    pub fn for_service(&self, service: &str) -> Self {
        Self(Arc::new(CtxInner {
            service: service.to_owned(),
            config: self.config.clone(),
            db: self.db.clone(),
            crypto: self.crypto.clone(),
            http: self.http.clone(),
            bridges: self.bridges.clone(),
        }))
    }

    /// Calls another service, in this process or over HTTP (`crate::bridge`). The URI of `req` is
    /// the path and query; the caller sets any timeout.
    ///
    /// # Errors
    /// [`Unreachable`] when the service has not answered; its error statuses are normal responses.
    pub async fn call(&self, service: &str, req: Request) -> Result<Response, Unreachable> {
        self.bridges.send(&self.config, service, req).await
    }

    /// A ready context for tests: in-memory database with migrations, random master key.
    ///
    /// # Errors
    /// When migrations fail.
    pub async fn for_tests(service: &str) -> anyhow::Result<Self> {
        Self::for_tests_with(service, &[]).await
    }

    /// Like [`Ctx::for_tests`], with extra config values (e.g. `("NOTES_URL", "http://…")`).
    ///
    /// # Errors
    /// When migrations fail.
    pub async fn for_tests_with(service: &str, vars: &[(&str, &str)]) -> anyhow::Result<Self> {
        let mut map: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        map.entry("MASTER_KEY".to_owned())
            .or_insert_with(crate::crypto::generate_master_key);
        let config = Config::from_vars(std::env::temp_dir(), map)?;
        Self::with_db(service, config, db::connect_memory().await?)
    }
}

/// Initializes tracing (`LOG_LEVEL` / `RUST_LOG`) once per process. `with_target` adds the module
/// to every line, which tells the services apart when they share a process.
pub fn init_tracing(config: &Config, with_target: bool) {
    let filter = EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));
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
    let router = finish(name, build(ctx.clone()).await?);

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("{name}: cannot listen on {addr}"))?;
    tracing::info!("{name} listening on http://{addr}");
    axum::serve(listener, router)
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
            axum::serve(listener, router)
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
        routers.insert((*name).to_owned(), finish(name, router));
    }
    ctx.bridges.connect(routers.clone());
    Ok(routers)
}

fn address(config: &Config, name: &str) -> String {
    format!("{}:{}", config.bind_host, config.port(name))
}

/// Adds `GET /health` and request tracing; the span names the service, so requests of services
/// sharing a process stay apart in the logs (a bridged call is a span inside the caller's one).
fn finish(name: &str, router: Router) -> Router {
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
