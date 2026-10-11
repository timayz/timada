//! Runtime configuration.
//!
//! Priority, highest first: `TIMADA__<SECTION>__<KEY>` environment variables,
//! the TOML file given with `--config` / `TIMADA_CONFIG`, then the defaults
//! set in [`Config::load`]. `config/default.toml` documents the full shape.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};

use config::{ConfigError, Environment, File};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    /// Absent: a single node on the in-memory transport. Present: one node of
    /// an Accord cluster over TCP.
    #[serde(default)]
    pub cluster: Option<ClusterConfig>,
    pub auth: AuthConfig,
    pub monitoring: MonitoringConfig,
}

/// Sign-in settings. The relying-party id and origin for passkeys and the
/// OIDC redirect URIs derive from `server.url`.
#[derive(Debug, Deserialize, Clone)]
pub struct AuthConfig {
    /// Encrypts the cookies that carry sign-in attempts still in progress
    /// (OIDC state and PKCE verifier, passkey challenges). Fed to the cookie
    /// crate's `Key::derive_from`, which needs at least 32 bytes. Sessions are
    /// not affected by a rotation: only in-flight sign-ins are interrupted.
    pub secret_key: String,
    /// How long a signed-in session stays valid, in seconds.
    pub session_lifetime_secs: u64,
    /// Relying-party name the browser shows when creating a passkey.
    pub rp_name: String,
    /// Sign-in providers keyed by the name used in URLs
    /// (`/auth/oauth2/<name>`). Sorted so the login page is stable.
    #[serde(default)]
    pub providers: BTreeMap<String, ProviderConfig>,
}

/// One OpenID Connect provider. Endpoints come from the issuer's discovery
/// document at startup.
#[derive(Debug, Deserialize, Clone)]
pub struct ProviderConfig {
    /// Button label, e.g. `Google`.
    pub label: String,
    /// Issuer URL, e.g. `https://accounts.google.com`.
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ServerConfig {
    /// Public base URL, logged at startup so it can be opened from the terminal.
    pub url: String,
    pub host: String,
    pub port: u16,
    /// Region this node serves, e.g. `eu-west-1`. Used as evento's default
    /// routing key: stamped on every new aggregate written here and inherited
    /// by this node's subscriptions (those that must see every region call
    /// `.any_routing_key()`). Absent: no routing key.
    #[serde(default)]
    pub region: Option<String>,
    /// Reverse proxies allowed to report the client's address through
    /// `X-Forwarded-For`, as IP addresses or CIDR networks. Empty: the
    /// connection's peer address is the client.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    /// sqlx DSN, e.g. `sqlite:data/timada.db`.
    pub url: String,
    /// Pool size for `serve`. sqlx-sqlite runs one OS thread per open
    /// connection, so keep it small.
    pub max_connections: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ClusterConfig {
    /// This node's index into `peers`.
    pub node_id: u64,
    /// Every node's Accord address, including this one. `NodeId(i)` is `peers[i]`.
    pub peers: Vec<SocketAddr>,
    /// Address to bind for inbound Accord traffic. Defaults to `peers[node_id]`;
    /// set it to `0.0.0.0:<port>` in a container.
    #[serde(default)]
    pub listen: Option<SocketAddr>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MonitoringConfig {
    /// A `tracing_subscriber::EnvFilter` directive, e.g. `info,sqlx=warn`.
    pub log_level: String,
    pub log_json: bool,
    pub log_target: bool,
    pub log_line_number: bool,
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Self, ConfigError> {
        let mut builder = config::Config::builder()
            .set_default("server.url", "https://timada.localhost")?
            // Traefik reaches the dev server through host.docker.internal, which
            // is not the loopback interface.
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            .set_default("database.url", "sqlite:data/timada.db")?
            .set_default("database.max_connections", 3)?
            .set_default("auth.session_lifetime_secs", 14 * 24 * 60 * 60)?
            .set_default("auth.rp_name", "Timada")?
            .set_default("monitoring.log_level", "info,sqlx=warn")?
            .set_default("monitoring.log_json", false)?
            .set_default("monitoring.log_target", true)?
            .set_default("monitoring.log_line_number", true)?;

        if let Some(path) = path {
            builder = builder.add_source(File::with_name(path).required(true));
        }

        let config: Self = builder
            .add_source(Environment::with_prefix("timada").separator("__"))
            .build()?
            .try_deserialize()?;

        if let Some(cluster) = &config.cluster {
            cluster.validate()?;
        }
        config.server.validate()?;
        config.auth.validate()?;

        Ok(config)
    }
}

impl ServerConfig {
    /// The router panics on an unparsable proxy entry, so reject it here with
    /// the offending value.
    fn validate(&self) -> Result<(), ConfigError> {
        for entry in &self.trusted_proxies {
            let valid =
                entry.parse::<IpAddr>().is_ok() || entry.parse::<topcoat::router::IpNet>().is_ok();
            if !valid {
                return Err(ConfigError::Message(format!(
                    "server.trusted_proxies: '{entry}' is not an IP address or a CIDR network"
                )));
            }
        }
        Ok(())
    }
}

impl AuthConfig {
    /// The cookie crate panics on a short key, so reject it here with a
    /// readable message. Provider names become URL segments.
    fn validate(&self) -> Result<(), ConfigError> {
        if self.secret_key.len() < 32 {
            return Err(ConfigError::Message(
                "auth.secret_key must be at least 32 characters (set TIMADA__AUTH__SECRET_KEY)"
                    .into(),
            ));
        }
        if self.session_lifetime_secs == 0 {
            return Err(ConfigError::Message(
                "auth.session_lifetime_secs must be greater than 0".into(),
            ));
        }
        for name in self.providers.keys() {
            let valid = !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if !valid {
                return Err(ConfigError::Message(format!(
                    "auth.providers.{name}: provider names use lowercase letters, digits, - and _"
                )));
            }
        }
        Ok(())
    }
}

impl ClusterConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.peers.is_empty() {
            return Err(ConfigError::Message(
                "cluster.peers must list at least one node".into(),
            ));
        }
        if self.node_id as usize >= self.peers.len() {
            return Err(ConfigError::Message(format!(
                "cluster.node_id {} is out of range for {} peers",
                self.node_id,
                self.peers.len()
            )));
        }
        Ok(())
    }

    pub fn listen_addr(&self) -> SocketAddr {
        self.listen.unwrap_or(self.peers[self.node_id as usize])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(trusted_proxies: &[&str]) -> ServerConfig {
        ServerConfig {
            url: "https://timada.localhost".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            region: None,
            trusted_proxies: trusted_proxies.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn trusted_proxies_accept_addresses_and_networks() {
        assert!(server(&[]).validate().is_ok());
        assert!(
            server(&["127.0.0.1", "172.16.0.0/12", "fd00::/8", "::1"])
                .validate()
                .is_ok()
        );
    }

    #[test]
    fn trusted_proxies_reject_garbage_with_the_value() {
        let err = server(&["10.0.0.0/8", "traefik"]).validate().unwrap_err();
        assert!(err.to_string().contains("'traefik'"), "{err}");
    }
}
