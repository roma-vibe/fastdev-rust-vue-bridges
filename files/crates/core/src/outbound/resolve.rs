//! DNS for outbound requests: the system resolver, minus every address [`is_blocked`] refuses.
//! reqwest connects only to the addresses returned here, so a name cannot be checked as public
//! and then connect somewhere private (no second lookup, no DNS rebinding window).

use std::{error::Error, fmt, net::SocketAddr};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

use super::addr::is_blocked;

/// The resolver given to the outbound client (`ClientBuilder::dns_resolver`).
#[derive(Debug, Default)]
pub struct PublicOnly;

impl Resolve for PublicOnly {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let found: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|addr| !is_blocked(addr.ip()))
                .collect();
            if found.is_empty() {
                return Err(Box::new(NoPublicAddress(host)) as Box<dyn Error + Send + Sync>);
            }
            Ok(Box::new(found.into_iter()) as Addrs)
        })
    }
}

/// A host name that resolves only to refused addresses (or to none).
#[derive(Debug)]
pub struct NoPublicAddress(pub String);

impl fmt::Display for NoPublicAddress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} has no public address", self.0)
    }
}

impl Error for NoPublicAddress {}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use super::*;

    #[tokio::test]
    async fn localhost_resolves_to_nothing_usable() {
        let name = Name::from_str("localhost").unwrap();
        let err = PublicOnly.resolve(name).await.err().unwrap();
        assert!(err.to_string().contains("no public address"), "{err}");
    }
}
