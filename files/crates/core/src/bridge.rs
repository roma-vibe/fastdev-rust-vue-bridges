//! Bridges: how services call each other (`docs/bridges.md`).
//!
//! A service never cares where another one runs. It calls [`Ctx::call`](crate::Ctx::call), and:
//!
//! - when this process hosts that service (`app-all`), the request goes straight into its router:
//!   no socket, no extra memory;
//! - otherwise it goes over HTTP to [`Config::service_url`], as between microservices.
//!
//! Both ways stream the bodies and the other service sees the same thing: a method, a URI, headers
//! and a body. So switching a bridge off changes no service code.

use std::{collections::HashMap, panic::AssertUnwindSafe, sync::OnceLock, time::Duration};

use axum::{
    Router,
    body::{Body, HttpBody as _},
    extract::Request,
    response::Response,
};
use futures::{FutureExt as _, TryStreamExt as _};
use tower::ServiceExt as _;

use crate::Config;

/// A service could not answer: it cannot be reached or it panicked.
#[derive(Debug, thiserror::Error)]
#[error("service {service} is unreachable: {detail}")]
pub struct Unreachable {
    pub service: String,
    pub detail: String,
}

/// The services hosted by this process, and the client for those in other processes.
#[derive(Debug, Default)]
pub struct Bridges {
    /// Empty in a process that runs one service; set once when `app-all` has built its services.
    local: OnceLock<HashMap<String, Router>>,
    /// Built on the first HTTP call: a process that hosts everything never needs it.
    client: OnceLock<reqwest::Client>,
}

impl Bridges {
    /// Connects the routers of the services this process hosts.
    pub(crate) fn connect(&self, routers: HashMap<String, Router>) {
        // Already connected means a second `connect` call: the first set of routers stays.
        let _ = self.local.set(routers);
    }

    /// Whether calls to `service` stay inside this process.
    #[must_use]
    pub fn is_local(&self, service: &str) -> bool {
        self.router(service).is_some()
    }

    fn router(&self, service: &str) -> Option<&Router> {
        self.local.get()?.get(service)
    }

    /// Sends `req` to `service`. Its URI is the path and query; the caller sets any timeout.
    ///
    /// # Errors
    /// [`Unreachable`] when the service has not answered; its error statuses are normal responses.
    pub async fn send(
        &self,
        config: &Config,
        service: &str,
        req: Request,
    ) -> Result<Response, Unreachable> {
        let unreachable = |detail: String| Unreachable {
            service: service.to_owned(),
            detail,
        };
        match self.router(service) {
            Some(router) => in_process(router, req)
                .await
                .map_err(|()| unreachable("the handler panicked".to_owned())),
            None => self
                .over_http(config, service, req)
                .await
                .map_err(|err| unreachable(err.to_string())),
        }
    }

    async fn over_http(
        &self,
        config: &Config,
        service: &str,
        req: Request,
    ) -> anyhow::Result<Response> {
        let (parts, body) = req.into_parts();
        let path = parts.uri.path_and_query().map_or("/", |p| p.as_str());
        let url = format!("{}{path}", config.service_url(service));
        let mut request = self
            .client()?
            .request(parts.method, &url)
            .headers(parts.headers);
        if body.size_hint().exact() != Some(0) {
            request = request.body(reqwest::Body::wrap_stream(body.into_data_stream()));
        }
        let upstream = request.send().await?;
        let mut response = Response::builder().status(upstream.status());
        if let Some(headers) = response.headers_mut() {
            headers.clone_from(upstream.headers());
        }
        let stream = upstream.bytes_stream().map_err(std::io::Error::other);
        Ok(response.body(Body::from_stream(stream))?)
    }

    fn client(&self) -> reqwest::Result<&reqwest::Client> {
        if let Some(client) = self.client.get() {
            return Ok(client);
        }
        let client = reqwest::Client::builder()
            // Redirects go back to the caller unchanged, as a reverse proxy must.
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()?;
        Ok(self.client.get_or_init(|| client))
    }
}

/// Runs the request in the caller's task, so dropping the call cancels the handler as closing a
/// connection does. A panic ends this call only, as it would end one request of a separate service.
async fn in_process(router: &Router, req: Request) -> Result<Response, ()> {
    let (parts, body) = req.into_parts();
    // Only what HTTP would carry: extensions of the caller's request must not reach the service.
    let mut clean = Request::new(body);
    *clean.method_mut() = parts.method;
    *clean.uri_mut() = parts.uri;
    *clean.headers_mut() = parts.headers;
    match AssertUnwindSafe(router.clone().oneshot(clean))
        .catch_unwind()
        .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(never)) => match never {},
        Err(_) => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::{
        body::{Body, to_bytes},
        extract::Request,
        http::{StatusCode, header},
        routing::{get, post},
    };
    use futures::StreamExt as _;
    use tokio::sync::mpsc;

    use super::*;

    fn config(vars: &[(&str, String)]) -> Config {
        let mut map = HashMap::new();
        map.insert("MASTER_KEY".to_owned(), "x".to_owned());
        for (key, value) in vars {
            map.insert((*key).to_owned(), value.clone());
        }
        Config::from_vars(std::env::temp_dir(), map).unwrap()
    }

    /// Echoes the method, the path and query, one header and the body.
    async fn echo(req: Request) -> String {
        let method = req.method().clone();
        let uri = req.uri().clone();
        let token = req.headers().get(header::AUTHORIZATION).cloned();
        let body = to_bytes(req.into_body(), usize::MAX).await.unwrap();
        format!(
            "{method} {uri} {:?} {}",
            token.as_ref().and_then(|t| t.to_str().ok()),
            String::from_utf8_lossy(&body)
        )
    }

    fn echo_request() -> Request {
        Request::post("/echo?a=1")
            .header(header::AUTHORIZATION, "Bearer t")
            .body(Body::from("hello"))
            .unwrap()
    }

    async fn text(response: Response) -> String {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn bridges_to(router: Router) -> Bridges {
        let bridges = Bridges::default();
        bridges.connect(HashMap::from([("notes".to_owned(), router)]));
        bridges
    }

    #[tokio::test]
    async fn in_process_and_http_deliver_the_same_request() {
        let router = Router::new().route("/echo", post(echo));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let served = router.clone();
        tokio::spawn(async move { axum::serve(listener, served).await });

        let bridged = bridges_to(router);
        assert!(bridged.is_local("notes"));
        let inside = bridged
            .send(&config(&[]), "notes", echo_request())
            .await
            .unwrap();

        let remote = Bridges::default();
        assert!(!remote.is_local("notes"));
        let over_http = remote
            .send(&config(&[("NOTES_URL", url)]), "notes", echo_request())
            .await
            .unwrap();

        assert_eq!(inside.status(), StatusCode::OK);
        assert_eq!(over_http.status(), StatusCode::OK);
        let inside = text(inside).await;
        assert_eq!(inside, r#"POST /echo?a=1 Some("Bearer t") hello"#);
        assert_eq!(text(over_http).await, inside);
    }

    #[tokio::test]
    async fn an_unknown_address_is_unreachable() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let config = config(&[("NOTES_URL", format!("http://127.0.0.1:{port}"))]);
        let err = Bridges::default()
            .send(&config, "notes", echo_request())
            .await
            .unwrap_err();
        assert_eq!(err.service, "notes");
    }

    #[tokio::test]
    async fn a_panicking_handler_ends_only_its_call() {
        async fn boom() -> &'static str {
            panic!("boom")
        }
        let bridges = bridges_to(Router::new().route("/boom", get(boom)));
        let req = Request::get("/boom").body(Body::empty()).unwrap();
        let err = bridges.send(&config(&[]), "notes", req).await.unwrap_err();
        assert!(err.detail.contains("panicked"), "{err}");
    }

    #[tokio::test]
    async fn the_response_streams_before_the_body_ends() {
        let (sender, receiver) = mpsc::channel::<Result<&'static str, std::io::Error>>(1);
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(Some(receiver)));
        let stream = move || {
            let receiver = receiver.lock().unwrap().take().unwrap();
            let chunks = futures::stream::unfold(receiver, |mut r| async move {
                r.recv().await.map(|chunk| (chunk, r))
            });
            std::future::ready(Body::from_stream(chunks))
        };
        let bridges = bridges_to(Router::new().route("/stream", get(stream)));
        let req = Request::get("/stream").body(Body::empty()).unwrap();
        // The handler has not sent a chunk yet, and the response is already here.
        let response = bridges.send(&config(&[]), "notes", req).await.unwrap();
        let mut body = response.into_body().into_data_stream();

        sender.send(Ok("first")).await.unwrap();
        assert_eq!(body.next().await.unwrap().unwrap(), "first");
        sender.send(Ok("second")).await.unwrap();
        assert_eq!(body.next().await.unwrap().unwrap(), "second");
        drop(sender);
        assert!(body.next().await.is_none());
    }

    #[tokio::test]
    async fn the_callers_extensions_stay_behind() {
        #[derive(Clone)]
        struct Secret;
        async fn has_secret(req: Request) -> String {
            req.extensions().get::<Secret>().is_some().to_string()
        }
        let bridges = bridges_to(Router::new().route("/x", get(has_secret)));
        let mut req = Request::get("/x").body(Body::empty()).unwrap();
        req.extensions_mut().insert(Secret);
        let response = bridges.send(&config(&[]), "notes", req).await.unwrap();
        assert_eq!(text(response).await, "false");
    }
}
