//! [`Ctx`]: what every service gets from the bootstrap (`super`).

use std::{collections::HashMap, ops::Deref, sync::Arc, time::Duration};

use axum::{extract::Request, response::Response};
use sqlx::SqlitePool;

use crate::{
    bridge::{Bridges, Unreachable},
    config::Config,
    crypto::Crypto,
    db,
    outbound::Outbound,
};

const USER_AGENT: &str = concat!("app/", env!("CARGO_PKG_VERSION"));

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
    /// Shared HTTP client for fixed, trusted API hosts written in the code (connection pooling);
    /// per-request timeouts are set by callers. Other services of this app are called with
    /// [`Ctx::call`], never with it.
    pub http: reqwest::Client,
    /// The SSRF-safe client for every URL from a user, a model, a webhook setting or a third
    /// party (`crate::outbound`).
    pub outbound: Outbound,
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
            .user_agent(USER_AGENT)
            .build()?;
        let outbound = Outbound::new(USER_AGENT)?;
        Ok(Self(Arc::new(CtxInner {
            service: service.to_owned(),
            config,
            db,
            crypto,
            http,
            outbound,
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
            outbound: self.outbound.clone(),
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
