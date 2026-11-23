//! SSRF-safe outbound HTTP: [`Outbound`], reachable as `ctx.outbound`. Use it for **every URL
//! that comes from a user, a model, a webhook setting or a third party**. It
//! - allows only `http`/`https` on ports 80 and 443 (more with [`Outbound::allow_ports`]),
//! - connects only to public addresses: host names go through [`resolve::PublicOnly`], IP literals
//!   are checked the same way ([`addr::is_blocked`]),
//! - follows at most [`MAX_REDIRECTS`] redirects by hand, checking every new URL,
//! - stops after [`TIMEOUT`] and at [`MAX_RESPONSE_BYTES`] of body (read as a stream).
//!
//! Other services of this app are called with `Ctx::call`; plain reqwest (`ctx.http`) is only for
//! fixed, trusted API hosts written in the code.

pub mod addr;
pub mod resolve;
#[cfg(test)]
mod tests;

use std::{net::IpAddr, sync::Arc, time::Duration};

use reqwest::{
    Method, StatusCode, Url,
    header::{
        AUTHORIZATION, COOKIE, HeaderMap, HeaderName, HeaderValue, LOCATION, PROXY_AUTHORIZATION,
    },
    redirect,
};

/// Redirects followed by hand (each one is checked like the first URL).
pub const MAX_REDIRECTS: usize = 3;
/// The whole exchange, redirects and body included.
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// The largest response body read.
pub const MAX_RESPONSE_BYTES: usize = 5 * 1024 * 1024;

/// Why an outbound request was refused or failed. The text is safe to log; show users only that
/// the URL could not be fetched.
#[derive(Debug, thiserror::Error)]
pub enum OutboundError {
    #[error("refused URL: {0}")]
    Refused(String),
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("response body over {MAX_RESPONSE_BYTES} bytes")]
    TooLarge,
    #[error("timed out")]
    Timeout,
    #[error(transparent)]
    Http(#[from] reqwest::Error),
}

/// A request: method, URL, headers and an optional body.
#[derive(Debug, Clone)]
pub struct OutboundRequest {
    method: Method,
    url: String,
    headers: HeaderMap,
    body: Option<Vec<u8>>,
}

impl OutboundRequest {
    #[must_use]
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            headers: HeaderMap::new(),
            body: None,
        }
    }

    #[must_use]
    pub fn header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// A JSON body (`content-type: application/json`).
    ///
    /// # Errors
    /// When `value` cannot be serialized.
    pub fn json<T: serde::Serialize>(mut self, value: &T) -> serde_json::Result<Self> {
        self.body = Some(serde_json::to_vec(value)?);
        self.headers.insert(
            reqwest::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        Ok(self)
    }
}

/// A finished response: the final URL after redirects, status, headers and the whole body.
#[derive(Debug)]
pub struct Fetched {
    pub url: Url,
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Vec<u8>,
}

/// The guarded client. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Outbound {
    client: reqwest::Client,
    ports: Arc<[u16]>,
}

impl Outbound {
    /// A client that resolves through [`resolve::PublicOnly`], ignores proxy settings (a proxy
    /// would resolve names itself) and never follows redirects on its own.
    ///
    /// # Errors
    /// When the TLS backend cannot be set up.
    pub fn new(user_agent: &str) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .dns_resolver(Arc::new(resolve::PublicOnly))
            .no_proxy()
            .redirect(redirect::Policy::none())
            .connect_timeout(TIMEOUT)
            .timeout(TIMEOUT)
            .user_agent(user_agent)
            .build()?;
        Ok(Self {
            client,
            ports: Arc::from([80, 443].as_slice()),
        })
    }

    /// The same client, also allowed to connect to `ports` (e.g. a self-hosted service on 8443).
    #[must_use]
    pub fn allow_ports(&self, ports: &[u16]) -> Self {
        let mut all = self.ports.to_vec();
        all.extend_from_slice(ports);
        Self {
            client: self.client.clone(),
            ports: Arc::from(all),
        }
    }

    /// `GET url`.
    ///
    /// # Errors
    /// See [`OutboundError`].
    pub async fn get(&self, url: &str) -> Result<Fetched, OutboundError> {
        self.fetch(OutboundRequest::new(Method::GET, url)).await
    }

    /// Sends `request`, following up to [`MAX_REDIRECTS`] checked redirects.
    ///
    /// # Errors
    /// See [`OutboundError`].
    pub async fn fetch(&self, request: OutboundRequest) -> Result<Fetched, OutboundError> {
        tokio::time::timeout(TIMEOUT, self.follow(request))
            .await
            .map_err(|_| OutboundError::Timeout)?
    }

    async fn follow(&self, request: OutboundRequest) -> Result<Fetched, OutboundError> {
        let OutboundRequest {
            mut method,
            url,
            mut headers,
            mut body,
        } = request;
        let mut url = self.check(&url)?;
        for _ in 0..=MAX_REDIRECTS {
            let mut builder = self
                .client
                .request(method.clone(), url.clone())
                .headers(headers.clone());
            if let Some(bytes) = &body {
                builder = builder.body(bytes.clone());
            }
            let response = builder.send().await?;
            let status = response.status();
            let Some(next) = redirect_target(&url, &response) else {
                return read(url, response).await;
            };
            let next = self.check(next.as_str())?;
            if next.origin() != url.origin() {
                for name in [AUTHORIZATION, COOKIE, PROXY_AUTHORIZATION] {
                    headers.remove(name);
                }
            }
            if status == StatusCode::SEE_OTHER
                || (matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND)
                    && method == Method::POST)
            {
                method = Method::GET;
                body = None;
                headers.remove(reqwest::header::CONTENT_TYPE);
            }
            url = next;
        }
        Err(OutboundError::TooManyRedirects)
    }

    /// Parses `raw` and refuses other schemes, ports, credentials and blocked IP literals.
    fn check(&self, raw: &str) -> Result<Url, OutboundError> {
        let refused = |why: &str| OutboundError::Refused(format!("{why}: {raw}"));
        let url = Url::parse(raw).map_err(|_| refused("not a URL"))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(refused("only http and https"));
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refused("credentials in the URL"));
        }
        let port = url
            .port_or_known_default()
            .ok_or_else(|| refused("no port"))?;
        if !self.ports.contains(&port) {
            return Err(refused("port not allowed"));
        }
        // The URL parser has already turned every IPv4 spelling (`2130706433`, `0x7f.1`) into
        // dotted decimal; IPv6 literals keep their brackets.
        let host = url.host_str().ok_or_else(|| refused("no host"))?;
        let literal = host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .ok();
        if literal.is_some_and(addr::is_blocked) {
            return Err(refused("address not allowed"));
        }
        Ok(url)
    }
}

/// The `Location` of a redirect response, resolved against `base`.
fn redirect_target(base: &Url, response: &reqwest::Response) -> Option<Url> {
    if !response.status().is_redirection() {
        return None;
    }
    let location = response.headers().get(LOCATION)?.to_str().ok()?;
    base.join(location).ok()
}

/// Reads the body as a stream, stopping at [`MAX_RESPONSE_BYTES`].
async fn read(url: Url, mut response: reqwest::Response) -> Result<Fetched, OutboundError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_RESPONSE_BYTES as u64)
    {
        return Err(OutboundError::TooLarge);
    }
    let status = response.status();
    let headers = std::mem::take(response.headers_mut());
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err(OutboundError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(Fetched {
        url,
        status,
        headers,
        body,
    })
}
