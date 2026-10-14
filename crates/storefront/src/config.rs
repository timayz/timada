use std::borrow::Cow;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use evento::RemoteAddress;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, ServerName};
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

/// Where the status endpoint lives unless the host chooses otherwise.
pub const DEFAULT_PREFIX: &str = "/store";

/// The client side of TLS to the store API.
#[derive(Clone)]
pub struct StoreTls {
    pub connector: TlsConnector,
    /// The name the store's certificate is checked against.
    pub server_name: ServerName<'static>,
}

impl StoreTls {
    /// Trusts the certificates of a PEM file (the store's own certificate,
    /// or the authority that signed it) for a store answering to
    /// `server_name`.
    pub fn from_ca_pem(path: &Path, server_name: &str) -> anyhow::Result<Self> {
        // rustls needs a process-wide crypto provider; installing twice is fine.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        let mut roots = RootCertStore::empty();
        for cert in CertificateDer::pem_file_iter(path)
            .with_context(|| format!("cannot read {}", path.display()))?
        {
            roots
                .add(cert.with_context(|| format!("cannot parse {}", path.display()))?)
                .context("the certificate is not usable as a trust anchor")?;
        }
        let config = ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Ok(Self {
            connector: TlsConnector::from(Arc::new(config)),
            server_name: ServerName::try_from(server_name.to_owned())
                .with_context(|| format!("{server_name:?} is not a valid server name"))?,
        })
    }
}

impl fmt::Debug for StoreTls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreTls")
            .field("server_name", &self.server_name)
            .finish_non_exhaustive()
    }
}

/// How to reach the store.
#[derive(Clone)]
pub struct StoreConfig {
    /// The store API: an IP and port, or a `host:port` looked up again on
    /// every reconnect, so a load balancer or a geo-routed name in front of
    /// several nodes is followed to whichever node answers.
    pub addr: RemoteAddress,
    /// The API key's token (`tk_…`), created on the tenant's API keys page.
    pub token: String,
    /// TLS to the store; plain TCP without (the token then travels in clear,
    /// for trusted networks only).
    pub tls: Option<StoreTls>,
    /// How long one request to the store may take.
    pub request_timeout: Duration,
    /// Where this tenant's product pictures are loaded from, with no
    /// trailing slash: the picture address shown on its API keys page.
    /// Without it, a picture is only its file name and the host serves it.
    pub media_url: Option<String>,
    /// The URL prefix of the library's own endpoints (`/store`).
    pub prefix: Cow<'static, str>,
}

impl StoreConfig {
    pub fn new(addr: impl Into<RemoteAddress>, token: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            token: token.into(),
            tls: None,
            request_timeout: Duration::from_secs(10),
            prefix: Cow::Borrowed(DEFAULT_PREFIX),
            media_url: None,
        }
    }

    pub fn tls(mut self, tls: StoreTls) -> Self {
        self.tls = Some(tls);
        self
    }

    pub fn request_timeout(mut self, timeout: Duration) -> Self {
        self.request_timeout = timeout;
        self
    }

    /// Where this tenant's product pictures are loaded from; a trailing
    /// slash is dropped. The tenant's API keys page shows the address.
    pub fn media_url(mut self, base: impl Into<String>) -> Self {
        let base = base.into();
        let base = base.trim_end_matches('/');
        self.media_url = (!base.is_empty()).then(|| base.to_owned());
        self
    }

    /// The URL prefix of the library's endpoints; a leading slash is added
    /// and a trailing one dropped.
    pub fn prefix(mut self, prefix: impl Into<Cow<'static, str>>) -> Self {
        let prefix = prefix.into();
        let trimmed = prefix.trim_matches('/');
        self.prefix = if trimmed.is_empty() {
            Cow::Borrowed("")
        } else if prefix.starts_with('/') && !prefix.ends_with('/') {
            prefix
        } else {
            Cow::Owned(format!("/{trimmed}"))
        };
        self
    }
}

impl fmt::Debug for StoreConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StoreConfig")
            .field("addr", &self.addr)
            .field("token", &"tk_…")
            .field("tls", &self.tls)
            .field("request_timeout", &self.request_timeout)
            .field("prefix", &self.prefix)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_are_normalized() {
        let config = StoreConfig::new("127.0.0.1:4321", "tk_x");
        assert_eq!(config.prefix, "/store");
        assert_eq!(config.clone().prefix("shop/").prefix, "/shop");
        assert_eq!(config.clone().prefix("/api/store").prefix, "/api/store");
        assert_eq!(config.clone().prefix("/").prefix, "");
        assert!(
            !format!("{config:?}").contains("tk_x"),
            "the token is never printed"
        );
    }
}
