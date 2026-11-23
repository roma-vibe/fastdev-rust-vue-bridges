//! Browser origins (`scheme://host[:port]`): the set of pages allowed to send state-changing
//! requests with the session cookie (`crate::auth`, the origin check).

use anyhow::bail;
use reqwest::Url;

/// The normalized origin of `url` (lowercase host, no default port, no path), e.g.
/// `https://example.com` for `https://Example.com:443/app/`. `None` for anything without one.
#[must_use]
pub fn origin_of(url: &str) -> Option<String> {
    let parsed = Url::parse(url.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    let origin = parsed.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

/// The allowed origins: that of `public_url`, the Vite dev server's (`localhost` and `127.0.0.1`
/// on `web_port`) when not in production, and every entry of the comma-separated `extra`
/// (`ALLOWED_ORIGINS`).
pub fn allowed(
    public_url: &str,
    web_port: &str,
    production: bool,
    extra: &str,
) -> anyhow::Result<Vec<String>> {
    let mut origins = Vec::new();
    let mut add = |origin: String| {
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    };
    match origin_of(public_url) {
        Some(origin) => add(origin),
        None => bail!("PUBLIC_URL must be an http(s) URL, got {public_url:?}"),
    }
    if !production && !web_port.is_empty() {
        add(format!("http://localhost:{web_port}"));
        add(format!("http://127.0.0.1:{web_port}"));
    }
    for entry in extra.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        match origin_of(entry) {
            Some(origin) => add(origin),
            None => bail!("ALLOWED_ORIGINS: {entry:?} is not an http(s) origin"),
        }
    }
    Ok(origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_normalized() {
        assert_eq!(
            origin_of("https://Example.COM:443/a?b").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("http://localhost:5173/").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(origin_of("null"), None);
        assert_eq!(origin_of("file:///etc/passwd"), None);
        assert_eq!(origin_of("javascript:alert(1)"), None);
    }
}
