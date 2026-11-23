//! The origin check against cross-site request forgery. Browsers send the session cookie with
//! every request to this host, also from other pages (another project on `localhost` is the same
//! site). So a state-changing request (`POST`, `PUT`, `PATCH`, `DELETE`) that is signed in by the
//! cookie must come from an allowed page: its `Origin` must be in `Config::allowed_origins`.
//! Programs that pass a key (`Authorization: Bearer`, `x-api-key`) are exempt: browsers never add
//! those by themselves.

use axum::http::{
    HeaderMap, Method,
    header::{AUTHORIZATION, ORIGIN},
};

use super::cookie::{read_cookie, session_cookie_name};
use crate::{
    config::{Config, origin_of},
    error::ApiError,
};

/// `Ok` unless the request changes state, carries the session cookie and no key, and its
/// `Origin` is missing or not allowed (then 403).
///
/// # Errors
/// [`ApiError::Forbidden`] for a refused request.
pub fn check_origin(method: &Method, headers: &HeaderMap, config: &Config) -> Result<(), ApiError> {
    let safe = matches!(
        *method,
        Method::GET | Method::HEAD | Method::OPTIONS | Method::TRACE
    );
    let has_key = headers.contains_key("x-api-key")
        || headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.len() > 7 && v[..7].eq_ignore_ascii_case("bearer "));
    if safe || has_key || read_cookie(headers, &session_cookie_name(config)).is_none() {
        return Ok(());
    }
    let origin = headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .and_then(origin_of);
    match origin {
        Some(origin) if config.allowed_origins.contains(&origin) => Ok(()),
        _ => Err(ApiError::forbidden(
            "Cross-site request refused: the Origin header is missing or not allowed",
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::HashMap, path::PathBuf};

    use axum::http::HeaderValue;

    use super::*;

    fn config() -> Config {
        let vars = HashMap::from([
            ("MASTER_KEY".to_owned(), "x".to_owned()),
            ("APP_SLUG".to_owned(), "app".to_owned()),
            ("PUBLIC_URL".to_owned(), "http://localhost:8700".to_owned()),
            (
                "ALLOWED_ORIGINS".to_owned(),
                "https://admin.example.com".to_owned(),
            ),
        ]);
        Config::from_vars(PathBuf::from("/"), vars).unwrap()
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        pairs
            .iter()
            .map(|(k, v)| (k.parse().unwrap(), HeaderValue::from_static(v)))
            .collect()
    }

    const COOKIE: (&str, &str) = ("cookie", "app-session=tok");

    #[test]
    fn cookie_writes_need_an_allowed_origin() {
        let config = config();
        for origin in [
            "http://localhost:8700",
            "https://admin.example.com",
            "http://LOCALHOST:8700",
        ] {
            let h = headers(&[COOKIE, ("origin", origin)]);
            assert!(check_origin(&Method::POST, &h, &config).is_ok(), "{origin}");
        }
        for origin in ["http://localhost:3000", "https://evil.example", "null"] {
            let h = headers(&[COOKIE, ("origin", origin)]);
            let err = check_origin(&Method::DELETE, &h, &config).unwrap_err();
            assert_eq!(err.status(), 403, "{origin}");
        }
        let missing = headers(&[COOKIE]);
        assert!(check_origin(&Method::PATCH, &missing, &config).is_err());
    }

    #[test]
    fn reads_keys_and_cookieless_requests_are_not_checked() {
        let config = config();
        let foreign = ("origin", "https://evil.example");
        assert!(check_origin(&Method::GET, &headers(&[COOKIE, foreign]), &config).is_ok());
        assert!(check_origin(&Method::POST, &headers(&[foreign]), &config).is_ok());
        let bearer = headers(&[COOKIE, foreign, ("authorization", "Bearer sk-1")]);
        assert!(check_origin(&Method::POST, &bearer, &config).is_ok());
        let api_key = headers(&[COOKIE, foreign, ("x-api-key", "sk-1")]);
        assert!(check_origin(&Method::PUT, &api_key, &config).is_ok());
        let basic = headers(&[COOKIE, foreign, ("authorization", "Basic eDp5")]);
        assert!(check_origin(&Method::POST, &basic, &config).is_err());
    }
}
