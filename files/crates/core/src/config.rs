//! Configuration: the root `.env` (and the process environment, which wins) read into [`Config`].
//! This is the only module that reads environment variables.

use std::{collections::HashMap, env, fmt, path::PathBuf};

use anyhow::{Context as _, bail};

use crate::{dotenv, registry};

#[derive(Clone)]
pub struct Config {
    /// Project root: the folder of `.env` (or of `services.json`), or `APP_ROOT`.
    pub root_dir: PathBuf,
    /// `APP_NAME`: the application name shown in the UI and returned by the API.
    pub app_name: String,
    /// `APP_SLUG`: machine-friendly name (cookie names, logs).
    pub app_slug: String,
    /// Absolute path of the SQLite file (`DB_PATH`, relative to the root).
    pub db_path: PathBuf,
    /// `MASTER_KEY` (32 bytes as hex or base64): encrypts secrets at rest. Never log it.
    pub master_key: String,
    /// Interface services listen on (`BIND_HOST`, default `127.0.0.1`).
    pub bind_host: String,
    /// Public URL users open in the browser (`PUBLIC_URL`); default: the Vite dev server or the edge.
    pub public_url: String,
    /// Built SPA served by the edge (`WEB_DIST`, default `web/dist`).
    pub web_dist: PathBuf,
    /// `LOG_LEVEL` or `RUST_LOG` filter, default `info`.
    pub log_level: String,
    /// `BRIDGES` (default on): off means no service runs inside `app-all` (`docs/bridges.md`).
    pub bridges: bool,
    /// `SEPARATE_SERVICES`: comma-separated services that keep a process of their own.
    separate: Vec<String>,
    /// Every key of `.env` and the environment, for service-specific options (`Config::var`).
    vars: HashMap<String, String>,
}

/// Secrets (`master_key`, `vars`) are never printed.
impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("root_dir", &self.root_dir)
            .field("app_name", &self.app_name)
            .field("db_path", &self.db_path)
            .field("bind_host", &self.bind_host)
            .field("public_url", &self.public_url)
            .field("bridges", &self.bridges)
            .field("separate", &self.separate)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Loads `.env` from the project root (found by walking up from the current directory), then
    /// overlays the process environment.
    ///
    /// # Errors
    /// When `.env` cannot be read or `MASTER_KEY` is missing.
    pub fn load() -> anyhow::Result<Self> {
        let root_dir = find_root()?;
        let mut vars: HashMap<String, String> = HashMap::new();
        let env_file = root_dir.join(".env");
        if env_file.exists() {
            let text = std::fs::read_to_string(&env_file)
                .with_context(|| format!("reading {}", env_file.display()))?;
            vars.extend(dotenv::parse(&text));
        }
        vars.extend(env::vars());
        Self::from_vars(root_dir, vars)
    }

    /// Builds a config from explicit values (tests).
    ///
    /// # Errors
    /// When `MASTER_KEY` is missing, `BRIDGES` is neither on nor off, or `SEPARATE_SERVICES` names
    /// an unknown service.
    pub fn from_vars(root_dir: PathBuf, vars: HashMap<String, String>) -> anyhow::Result<Self> {
        let get = |key: &str, default: &str| {
            vars.get(key)
                .filter(|v| !v.is_empty())
                .cloned()
                .unwrap_or_else(|| default.to_owned())
        };
        let Some(master_key) = vars.get("MASTER_KEY").filter(|v| !v.is_empty()).cloned() else {
            bail!("MASTER_KEY is not set: run `npm run setup` to create .env with a fresh key");
        };
        let resolve = |p: String| {
            let path = PathBuf::from(p);
            if path.is_absolute() {
                path
            } else {
                root_dir.join(path)
            }
        };
        let edge_port = registry::service("edge")
            .map(|s| get(&s.port_env, &s.default_port.to_string()))
            .unwrap_or_else(|| "8700".to_owned());
        let web_port = get("WEB_PORT", "");
        let default_public = if web_port.is_empty() {
            format!("http://localhost:{edge_port}")
        } else {
            format!("http://localhost:{web_port}")
        };
        let bridges = match get("BRIDGES", "on").to_lowercase().as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            other => bail!("BRIDGES must be on or off, got {other:?}"),
        };
        let separate: Vec<String> = get("SEPARATE_SERVICES", "")
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
            .collect();
        if let Some(unknown) = separate.iter().find(|n| registry::service(n).is_none()) {
            bail!("SEPARATE_SERVICES: unknown service {unknown:?} (see services.json)");
        }
        Ok(Self {
            app_name: get("APP_NAME", "App"),
            app_slug: get("APP_SLUG", "app"),
            db_path: resolve(get("DB_PATH", "data/app.sqlite")),
            master_key,
            bind_host: get("BIND_HOST", "127.0.0.1"),
            public_url: get("PUBLIC_URL", &default_public)
                .trim_end_matches('/')
                .to_owned(),
            web_dist: resolve(get("WEB_DIST", "web/dist")),
            log_level: vars
                .get("RUST_LOG")
                .or_else(|| vars.get("LOG_LEVEL"))
                .filter(|v| !v.is_empty())
                .cloned()
                .unwrap_or_else(|| "info".to_owned()),
            bridges,
            separate,
            root_dir,
            vars,
        })
    }

    /// Any `.env` / environment value, for options specific to one service.
    #[must_use]
    pub fn var(&self, key: &str) -> Option<&str> {
        self.vars
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// The port of service `name`: its `portEnv` from `.env`, else `defaultPort` from `services.json`.
    ///
    /// # Panics
    /// If `name` is not in `services.json` (a programming error).
    #[must_use]
    pub fn port(&self, name: &str) -> u16 {
        let info = registry::service(name)
            .unwrap_or_else(|| panic!("service {name:?} is not in services.json"));
        self.var(&info.port_env)
            .and_then(|v| v.parse().ok())
            .unwrap_or(info.default_port)
    }

    /// Services that run in a process of their own instead of inside `app-all`: all of them with
    /// `BRIDGES=off`, else `SEPARATE_SERVICES`. `scripts/lib/topology.ts` applies the same rule.
    #[must_use]
    pub fn separate_services(&self) -> Vec<&str> {
        if self.bridges {
            self.separate.iter().map(String::as_str).collect()
        } else {
            registry::services()
                .iter()
                .map(|s| s.name.as_str())
                .collect()
        }
    }

    /// Tokio workers of a process (`WORKER_THREADS`, default the CPU cores, at most 4): the services
    /// only wait for the network and the database, and hash passwords on blocking threads.
    #[must_use]
    pub fn worker_threads(&self) -> usize {
        self.var("WORKER_THREADS")
            .and_then(|v| v.parse().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get().min(4)))
    }

    /// Internal base URL of service `name` for service-to-service calls, e.g. `http://127.0.0.1:8701`.
    /// Override with `<NAME>_URL` (e.g. `NOTES_URL`) when a service runs on another host. Only the
    /// bridges (`crate::bridge`) use it: services call each other with `Ctx::call`.
    #[must_use]
    pub fn service_url(&self, name: &str) -> String {
        let key = format!("{}_URL", name.to_uppercase().replace('-', "_"));
        if let Some(url) = self.var(&key) {
            return url.trim_end_matches('/').to_owned();
        }
        let host = if self.bind_host == "0.0.0.0" {
            "127.0.0.1"
        } else {
            &self.bind_host
        };
        format!("http://{host}:{}", self.port(name))
    }
}

fn find_root() -> anyhow::Result<PathBuf> {
    if let Ok(root) = env::var("APP_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let start = env::current_dir().context("reading the current directory")?;
    for dir in start.ancestors() {
        if dir.join(".env").exists() || dir.join("services.json").exists() {
            return Ok(dir.to_path_buf());
        }
    }
    Ok(start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn ports_and_urls_come_from_the_registry() {
        let config = Config::from_vars(
            PathBuf::from("/tmp/root"),
            vars(&[("MASTER_KEY", "x"), ("EDGE_PORT", "9100")]),
        )
        .unwrap();
        assert_eq!(config.port("edge"), 9100);
        assert_eq!(config.service_url("edge"), "http://127.0.0.1:9100");
        assert_eq!(config.public_url, "http://localhost:9100");
        assert_eq!(config.db_path, PathBuf::from("/tmp/root/data/app.sqlite"));
    }

    #[test]
    fn service_urls_can_be_overridden() {
        let config = Config::from_vars(
            PathBuf::from("/"),
            vars(&[("MASTER_KEY", "x"), ("EDGE_URL", "http://edge:1/")]),
        )
        .unwrap();
        assert_eq!(config.service_url("edge"), "http://edge:1");
    }

    #[test]
    fn bridges_decide_what_runs_separately() {
        let load = |pairs: &[(&str, &str)]| {
            let mut all = vec![("MASTER_KEY", "x")];
            all.extend_from_slice(pairs);
            Config::from_vars(PathBuf::from("/"), vars(&all))
        };
        assert!(load(&[]).unwrap().bridges);
        assert!(load(&[]).unwrap().separate_services().is_empty());
        assert_eq!(
            load(&[("SEPARATE_SERVICES", "notes, edge")])
                .unwrap()
                .separate_services(),
            ["notes", "edge"]
        );
        let off = load(&[("BRIDGES", "off"), ("SEPARATE_SERVICES", "notes")]).unwrap();
        assert_eq!(off.separate_services().len(), registry::services().len());
        assert!(load(&[("BRIDGES", "maybe")]).is_err());
        assert!(load(&[("SEPARATE_SERVICES", "nope")]).is_err());
    }

    #[test]
    fn worker_threads_are_capped() {
        let load = |pairs: &[(&str, &str)]| {
            let mut all = vec![("MASTER_KEY", "x")];
            all.extend_from_slice(pairs);
            Config::from_vars(PathBuf::from("/"), vars(&all)).unwrap()
        };
        assert!((1..=4).contains(&load(&[]).worker_threads()));
        assert_eq!(load(&[("WORKER_THREADS", "2")]).worker_threads(), 2);
        assert!(load(&[("WORKER_THREADS", "0")]).worker_threads() >= 1);
    }

    #[test]
    fn requires_master_key_and_hides_it() {
        assert!(Config::from_vars(PathBuf::from("/"), HashMap::new()).is_err());
        let config =
            Config::from_vars(PathBuf::from("/"), vars(&[("MASTER_KEY", "top-secret")])).unwrap();
        assert!(!format!("{config:?}").contains("top-secret"));
    }
}
