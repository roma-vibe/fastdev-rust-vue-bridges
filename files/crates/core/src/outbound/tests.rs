use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use tokio::{io::AsyncWriteExt as _, net::TcpListener};

use super::*;

fn client() -> Outbound {
    Outbound::new("test").unwrap()
}

/// A local HTTP server that answers every connection with 200 and counts them.
async fn local_server() -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            counter.fetch_add(1, Ordering::SeqCst);
            let _ = socket
                .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                .await;
        }
    });
    (port, hits)
}

fn refused(result: Result<Fetched, OutboundError>) -> String {
    match result {
        Err(OutboundError::Refused(why)) => why,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_local_server_is_never_reached() {
    let (port, hits) = local_server().await;
    let outbound = client().allow_ports(&[port]);
    for url in [
        format!("http://127.0.0.1:{port}/"),
        format!("http://[::ffff:127.0.0.1]:{port}/"),
        format!("http://2130706433:{port}/"),
        format!("http://0x7f.1:{port}/"),
    ] {
        let why = refused(outbound.get(&url).await);
        assert!(why.contains("address not allowed"), "{url}: {why}");
    }
    // A name that resolves to loopback fails in the resolver, before any connection.
    let by_name = outbound.get(&format!("http://localhost:{port}/")).await;
    assert!(
        matches!(by_name, Err(OutboundError::Http(_))),
        "{by_name:?}"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "the local server was contacted"
    );
}

#[tokio::test]
async fn only_http_on_the_allowed_ports() {
    let outbound = client();
    assert!(refused(outbound.get("file:///etc/passwd").await).contains("only http"));
    assert!(refused(outbound.get("ftp://example.com/").await).contains("only http"));
    assert!(refused(outbound.get("gopher://example.com/").await).contains("only http"));
    assert!(refused(outbound.get("http://example.com:22/").await).contains("port"));
    assert!(refused(outbound.get("http://user:pw@example.com/").await).contains("credentials"));
    assert!(refused(outbound.get("not a url").await).contains("not a URL"));
    assert!(refused(outbound.get("http://169.254.169.254/latest/").await).contains("address"));
    assert!(refused(outbound.get("http://[fd00::1]/").await).contains("address"));
}

#[test]
fn redirects_are_resolved_and_checked_like_the_first_url() {
    let outbound = client();
    let base = Url::parse("https://example.com/a/b").unwrap();
    let next = base.join("/internal").unwrap();
    assert!(outbound.check(next.as_str()).is_ok());
    let local = base.join("http://127.0.0.1/admin").unwrap();
    assert!(outbound.check(local.as_str()).is_err());
}
