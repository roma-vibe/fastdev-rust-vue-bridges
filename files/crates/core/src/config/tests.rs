use super::*;

fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn load(pairs: &[(&str, &str)]) -> anyhow::Result<Config> {
    let mut all = vec![("MASTER_KEY", "x")];
    all.extend_from_slice(pairs);
    Config::from_vars(PathBuf::from("/"), vars(&all))
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
    let config = load(&[("EDGE_URL", "http://edge:1/")]).unwrap();
    assert_eq!(config.service_url("edge"), "http://edge:1");
}

#[test]
fn bridges_decide_what_runs_separately() {
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
    assert!((1..=4).contains(&load(&[]).unwrap().worker_threads()));
    assert_eq!(
        load(&[("WORKER_THREADS", "2")]).unwrap().worker_threads(),
        2
    );
    assert!(load(&[("WORKER_THREADS", "0")]).unwrap().worker_threads() >= 1);
}

#[test]
fn requires_master_key_and_hides_it() {
    assert!(Config::from_vars(PathBuf::from("/"), HashMap::new()).is_err());
    let config =
        Config::from_vars(PathBuf::from("/"), vars(&[("MASTER_KEY", "top-secret")])).unwrap();
    assert!(!format!("{config:?}").contains("top-secret"));
}

#[test]
fn production_follows_app_env_and_moves_the_default_public_url() {
    let dev = load(&[("APP_ENV", "development"), ("WEB_PORT", "5173")]).unwrap();
    assert!(!dev.production);
    assert_eq!(dev.public_url, "http://localhost:5173");
    let prod = load(&[("APP_ENV", "production"), ("WEB_PORT", "5173")]).unwrap();
    assert!(prod.production);
    assert_eq!(
        prod.public_url, "http://localhost:8700",
        "the edge serves the app"
    );
    assert!(load(&[("APP_ENV", "staging")]).is_err());
}

#[test]
fn allowed_origins_are_the_public_url_vite_in_development_and_the_extra_list() {
    let dev = load(&[
        ("APP_ENV", "development"),
        ("WEB_PORT", "5173"),
        ("PUBLIC_URL", "https://App.Example.com/"),
        (
            "ALLOWED_ORIGINS",
            "https://admin.example.com, http://localhost:3000/",
        ),
    ])
    .unwrap();
    assert_eq!(
        dev.allowed_origins,
        [
            "https://app.example.com",
            "http://localhost:5173",
            "http://127.0.0.1:5173",
            "https://admin.example.com",
            "http://localhost:3000",
        ]
    );
    let prod = load(&[
        ("APP_ENV", "production"),
        ("WEB_PORT", "5173"),
        ("PUBLIC_URL", "https://app.example.com:443"),
    ])
    .unwrap();
    assert_eq!(prod.allowed_origins, ["https://app.example.com"]);
    assert!(load(&[("ALLOWED_ORIGINS", "not a url")]).is_err());
}

#[test]
fn browser_facing_limits_and_proxy_trust() {
    let defaults = load(&[]).unwrap();
    assert_eq!(defaults.max_body_bytes, 1024 * 1024);
    assert!(
        !defaults.trust_proxy,
        "no proxy is trusted unless .env says so"
    );
    let set = load(&[("MAX_BODY_BYTES", "2048"), ("TRUST_PROXY", "on")]).unwrap();
    assert_eq!(set.max_body_bytes, 2048);
    assert!(set.trust_proxy);
    assert!(load(&[("MAX_BODY_BYTES", "lots")]).is_err());
    assert!(load(&[("TRUST_PROXY", "perhaps")]).is_err());
}
