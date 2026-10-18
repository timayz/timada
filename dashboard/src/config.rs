//! Runtime configuration.
//!
//! Priority, highest first: `TIMADA__<SECTION>__<KEY>` environment variables,
//! the TOML file given with `--config` / `TIMADA_CONFIG`, then the defaults
//! set in [`Config::load`]. `config/default.toml` documents the full shape.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use config::{ConfigError, Environment, File};
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    /// Absent: a single node, talking to itself over a loopback port. Present:
    /// one node of an Accord cluster over TCP.
    #[serde(default)]
    pub cluster: Option<ClusterConfig>,
    pub tenants: TenantsConfig,
    pub media: MediaConfig,
    /// `[data_keys]`; `[shopper_keys]`, its first name, still reads.
    #[serde(alias = "shopper_keys")]
    pub data_keys: DataKeysConfig,
    /// Absent: this node does not expose the store API.
    #[serde(default)]
    pub store_api: Option<StoreApiConfig>,
    /// Absent: tenants cannot take payments.
    #[serde(default)]
    pub payments: Option<PaymentsConfig>,
    /// Absent: no tenant secret can be stored (shop mail servers, shopper
    /// accounts, and payments unless `payments.secret_key` is set).
    #[serde(default)]
    pub secrets: Option<SecretsConfig>,
    /// Invoices; every setting has a default.
    #[serde(default)]
    pub invoicing: InvoicingConfig,
    /// Reviews; every setting has a default.
    #[serde(default)]
    pub reviews: ReviewsConfig,
    /// What storefronts may ask for their shoppers; every setting has a
    /// default.
    #[serde(default)]
    pub store: StoreConfig,
    /// How many invitations members may send; every setting has a default.
    #[serde(default)]
    pub invitations: InvitationsConfig,
    /// Products imported from AliExpress; every setting has a default.
    #[serde(default)]
    pub aliexpress: AliexpressConfig,
    pub auth: AuthConfig,
    pub mail: MailConfig,
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

/// Outgoing e-mail: the codes that prove an address at sign-up, sign-in and
/// password reset, and the shops' mails to their shoppers when a shop has no
/// mail server of its own.
#[derive(Debug, Deserialize, Clone)]
pub struct MailConfig {
    /// Sender shown on those e-mails, e.g. `Timada <no-reply@example.com>`.
    pub from: String,
    /// Where messages are submitted: `smtp://host:port` (no encryption, for a
    /// local mailbox), `smtp://user:pass@host:587?tls=required` (STARTTLS) or
    /// `smtps://user:pass@host:465` (implicit TLS).
    pub smtp_url: String,
    /// Lets a shop's mail server be on this machine or a private network.
    /// Off by default: a merchant could otherwise make the server connect to
    /// internal services. For development and tests only.
    #[serde(default)]
    pub allow_private_hosts: bool,
}

/// The key that seals the secrets tenants hand over (their mail server
/// passwords, their payment keys) before they are stored.
#[derive(Debug, Deserialize, Clone)]
pub struct SecretsConfig {
    /// At least 32 characters, the same on every node. Changing it makes the
    /// stored secrets unreadable, and each merchant has to enter them again.
    pub key: String,
}

impl SecretsConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.key.len() < 32 {
            return Err(ConfigError::Message(
                "secrets.key must be at least 32 characters (set TIMADA__SECRETS__KEY)".into(),
            ));
        }
        Ok(())
    }
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

/// Where tenant databases live and how many stay open at once.
#[derive(Debug, Deserialize, Clone)]
pub struct TenantsConfig {
    /// Directory holding one SQLite file per tenant (its events and its
    /// consensus journal).
    pub dir: PathBuf,
    /// Most tenant databases open at once; beyond it the least recently used
    /// is closed. Each open database holds up to three connections, and
    /// sqlx-sqlite runs one OS thread per connection. Unset: worked out from
    /// the machine at startup ([`crate::limits::auto_max_open`]).
    pub max_open: Option<usize>,
}

/// The keys that open personal data (each shopper account's, each contact
/// message's): one file per key, copied to every node and sealed with
/// `secrets.key`. Deleting an account or erasing a message deletes its key,
/// which leaves what it recorded unreadable for good.
#[derive(Debug, Deserialize, Clone)]
pub struct DataKeysConfig {
    /// Directory holding one sub-directory per tenant. Created on demand.
    pub dir: PathBuf,
}

/// The pictures merchants upload for their products: where the files are
/// kept and the address shoppers load them from.
#[derive(Debug, Deserialize, Clone)]
pub struct MediaConfig {
    /// Directory holding one sub-directory per tenant. Created on demand.
    pub dir: PathBuf,
    /// The address this node's `/media` is reached at from the outside, or a
    /// cache in front of it. A storefront is told to load pictures from here,
    /// so it has to work from a shopper's browser, not only from the server.
    pub public_url: String,
}

impl MediaConfig {
    fn validate(&mut self) -> Result<(), ConfigError> {
        let url = self.public_url.trim_end_matches('/');
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ConfigError::Message(format!(
                "media.public_url: '{}' must start with http:// or https://",
                self.public_url
            )));
        }
        self.public_url = url.to_owned();
        Ok(())
    }
}

/// Reviews.
#[derive(Debug, Deserialize, Clone)]
pub struct ReviewsConfig {
    /// How often, in seconds, this node looks for fulfilled orders whose
    /// buyer is due a mail asking for a review.
    #[serde(default = "default_request_check_secs")]
    pub request_check_secs: u64,
}

impl Default for ReviewsConfig {
    fn default() -> Self {
        Self {
            request_check_secs: default_request_check_secs(),
        }
    }
}

fn default_request_check_secs() -> u64 {
    60 * 60
}

/// What storefronts may ask of this node for their shoppers. Each node counts
/// on its own: behind a balancer over N nodes, a client may get up to N
/// times these limits.
#[derive(Debug, Deserialize, Clone)]
pub struct StoreConfig {
    /// Sign-in codes one shopper's address may have mailed, at one shop,
    /// within the window.
    #[serde(default = "default_shopper_codes_per_ip")]
    pub shopper_codes_per_ip: u32,
    /// Sign-in codes one API key may have mailed within the window, whoever
    /// the shoppers.
    #[serde(default = "default_shopper_codes_per_key")]
    pub shopper_codes_per_key: u32,
    /// The window both limits count over, in seconds.
    #[serde(default = "default_shopper_codes_window_secs")]
    pub shopper_codes_window_secs: u64,
    /// Messages one visitor's address may send through the shop's contact
    /// form, at one shop, within the window.
    #[serde(default = "default_contact_messages_per_ip")]
    pub contact_messages_per_ip: u32,
    /// Messages one API key may hand in within the window, whoever the
    /// visitors.
    #[serde(default = "default_contact_messages_per_key")]
    pub contact_messages_per_key: u32,
    /// The window both message limits count over, in seconds.
    #[serde(default = "default_contact_messages_window_secs")]
    pub contact_messages_window_secs: u64,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            shopper_codes_per_ip: default_shopper_codes_per_ip(),
            shopper_codes_per_key: default_shopper_codes_per_key(),
            shopper_codes_window_secs: default_shopper_codes_window_secs(),
            contact_messages_per_ip: default_contact_messages_per_ip(),
            contact_messages_per_key: default_contact_messages_per_key(),
            contact_messages_window_secs: default_contact_messages_window_secs(),
        }
    }
}

fn default_shopper_codes_per_ip() -> u32 {
    10
}

fn default_shopper_codes_per_key() -> u32 {
    100
}

fn default_shopper_codes_window_secs() -> u64 {
    10 * 60
}

fn default_contact_messages_per_ip() -> u32 {
    5
}

fn default_contact_messages_per_key() -> u32 {
    200
}

fn default_contact_messages_window_secs() -> u64 {
    60 * 60
}

impl StoreConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        for (name, value) in [
            (
                "store.shopper_codes_per_ip",
                u64::from(self.shopper_codes_per_ip),
            ),
            (
                "store.shopper_codes_per_key",
                u64::from(self.shopper_codes_per_key),
            ),
            (
                "store.shopper_codes_window_secs",
                self.shopper_codes_window_secs,
            ),
            (
                "store.contact_messages_per_ip",
                u64::from(self.contact_messages_per_ip),
            ),
            (
                "store.contact_messages_per_key",
                u64::from(self.contact_messages_per_key),
            ),
            (
                "store.contact_messages_window_secs",
                self.contact_messages_window_secs,
            ),
        ] {
            if value == 0 {
                return Err(ConfigError::Message(format!(
                    "{name} must be greater than 0"
                )));
            }
        }
        Ok(())
    }
}

/// How many invitations may be mailed. Counted from what every node has
/// recorded, so the limits hold across a cluster and across restarts.
#[derive(Debug, Deserialize, Clone)]
pub struct InvitationsConfig {
    /// Invitations one tenant may send within the window, whoever sends them.
    #[serde(default = "default_invitations_per_tenant")]
    pub per_tenant: u32,
    /// Invitations one account may send within the window, across every
    /// tenant it belongs to.
    #[serde(default = "default_invitations_per_account")]
    pub per_account: u32,
    /// The window both limits count over, in seconds.
    #[serde(default = "default_invitations_window_secs")]
    pub window_secs: u64,
}

impl Default for InvitationsConfig {
    fn default() -> Self {
        Self {
            per_tenant: default_invitations_per_tenant(),
            per_account: default_invitations_per_account(),
            window_secs: default_invitations_window_secs(),
        }
    }
}

fn default_invitations_per_tenant() -> u32 {
    50
}

fn default_invitations_per_account() -> u32 {
    20
}

fn default_invitations_window_secs() -> u64 {
    24 * 3600
}

impl InvitationsConfig {
    /// Whether one more invitation fits, given those sent within the window.
    pub fn allows(&self, sent: &timada_tenancy::SentCounts) -> bool {
        sent.tenant < i64::from(self.per_tenant) && sent.account < i64::from(self.per_account)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        for (name, value) in [
            ("invitations.per_tenant", u64::from(self.per_tenant)),
            ("invitations.per_account", u64::from(self.per_account)),
            ("invitations.window_secs", self.window_secs),
        ] {
            if value == 0 {
                return Err(ConfigError::Message(format!(
                    "{name} must be greater than 0"
                )));
            }
        }
        Ok(())
    }
}

/// Products a shop imports from AliExpress through its own app. The app's
/// secret and tokens are kept encrypted under `secrets.key`: without it, no
/// shop can connect.
#[derive(Debug, Deserialize, Clone)]
pub struct AliexpressConfig {
    /// Where AliExpress's open platform is reached; its own address when
    /// unset. Tests point it at a stand-in.
    #[serde(default)]
    pub api_url: Option<String>,
    /// Where a merchant's AliExpress account says yes to their app;
    /// AliExpress's own page when unset.
    #[serde(default)]
    pub authorize_url: Option<String>,
    /// How often, in seconds, a node looks for imported products due to be
    /// read again.
    #[serde(default = "default_aliexpress_check_secs")]
    pub sync_check_secs: u64,
    /// How long, in seconds, an imported product goes before its price and
    /// stock are read again.
    #[serde(default = "default_aliexpress_every_secs")]
    pub sync_every_secs: u64,
}

fn default_aliexpress_check_secs() -> u64 {
    60 * 60
}

fn default_aliexpress_every_secs() -> u64 {
    6 * 60 * 60
}

impl Default for AliexpressConfig {
    fn default() -> Self {
        Self {
            api_url: None,
            authorize_url: None,
            sync_check_secs: default_aliexpress_check_secs(),
            sync_every_secs: default_aliexpress_every_secs(),
        }
    }
}

/// Invoices.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct InvoicingConfig {
    /// Where the EU's VIES service checks business buyers' VAT numbers; the
    /// European Commission's own when unset. Tests point it at a stand-in.
    #[serde(default)]
    pub vies_url: Option<String>,
}

/// Card payments: each tenant connects its own Stripe account; this node
/// keeps the account's secrets encrypted under `secret_key`, or else under
/// `secrets.key`.
#[derive(Debug, Deserialize, Clone)]
pub struct PaymentsConfig {
    /// At least 32 characters, the same on every node. The tenants' Stripe
    /// secrets are encrypted with it: changing it makes them unreadable, and
    /// each merchant has to enter them again. Unset: `secrets.key` is used.
    #[serde(default)]
    pub secret_key: Option<String>,
    /// Where Stripe's API is reached; Stripe's own address when unset. Tests
    /// point it at a local stand-in.
    #[serde(default)]
    pub stripe_api_url: Option<String>,
}

impl PaymentsConfig {
    fn validate(&self, secrets: Option<&SecretsConfig>) -> Result<(), ConfigError> {
        match &self.secret_key {
            Some(key) if key.len() < 32 => Err(ConfigError::Message(
                "payments.secret_key must be at least 32 characters (set TIMADA__PAYMENTS__SECRET_KEY)"
                    .into(),
            )),
            None if secrets.is_none() => Err(ConfigError::Message(
                "payments needs payments.secret_key or secrets.key (set TIMADA__SECRETS__KEY)"
                    .into(),
            )),
            _ => Ok(()),
        }
    }
}

/// The store API: where a merchant's storefront connects with one of the
/// tenant's API keys to read and write that tenant's events.
#[derive(Debug, Deserialize, Clone)]
pub struct StoreApiConfig {
    /// Address to bind, e.g. `0.0.0.0:4321`.
    pub listen: SocketAddr,
    /// The address storefronts are told to connect to, shown on the API keys
    /// page next to a new key.
    #[serde(default)]
    pub public_addr: Option<String>,
    /// PEM files; with both set, connections are encrypted. Without them a
    /// key travels in clear text: only on a private network or behind a
    /// proxy that terminates TLS.
    #[serde(default)]
    pub tls_cert: Option<PathBuf>,
    #[serde(default)]
    pub tls_key: Option<PathBuf>,
}

impl StoreApiConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        match (&self.tls_cert, &self.tls_key) {
            (None, None) => Ok(()),
            (Some(cert), Some(key)) => {
                for (name, path) in [("store_api.tls_cert", cert), ("store_api.tls_key", key)] {
                    if !path.is_file() {
                        return Err(ConfigError::Message(format!(
                            "{name}: '{}' is not a readable file",
                            path.display()
                        )));
                    }
                }
                Ok(())
            }
            _ => Err(ConfigError::Message(
                "store_api.tls_cert and store_api.tls_key go together: set both or neither".into(),
            )),
        }
    }

    /// The certificate and key files, when connections are encrypted.
    pub fn tls(&self) -> Option<(&Path, &Path)> {
        Some((self.tls_cert.as_deref()?, self.tls_key.as_deref()?))
    }
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
    /// Every node's web address as the other nodes reach it, in `peers`
    /// order, e.g. `http://10.0.0.2:3000`. Product pictures are copied
    /// between nodes through it. Required with more than one node.
    #[serde(default)]
    pub web_peers: Vec<String>,
    /// Shared by every node: a node only accepts a picture copied from a
    /// peer that presents it. At least 32 characters; required with more
    /// than one node.
    #[serde(default)]
    pub secret: Option<String>,
    /// A node not heard from on a tenant for this long is no longer waited
    /// for: the others tidy up finished writes without it. When it returns,
    /// every tenant it fell behind on is copied back from a peer. 0 never
    /// gives a node up.
    #[serde(default = "default_absent_after_secs")]
    pub absent_after_secs: u64,
    /// Finished writes a tenant keeps for a node that is away. Past it the
    /// tenant refuses new writes until the node returns or is given up.
    #[serde(default = "default_write_backlog")]
    pub write_backlog: usize,
}

fn default_absent_after_secs() -> u64 {
    3600
}

fn default_write_backlog() -> usize {
    100_000
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
            .set_default("tenants.dir", "data/tenants")?
            .set_default("media.dir", "data/media")?
            .set_default("media.public_url", "https://timada.localhost/media")?
            .set_default("data_keys.dir", "data/shopper-keys")?
            .set_default("auth.session_lifetime_secs", 14 * 24 * 60 * 60)?
            .set_default("auth.rp_name", "Timada")?
            .set_default("mail.from", "Timada <no-reply@timada.localhost>")?
            .set_default("mail.smtp_url", "smtp://localhost:1025")?
            .set_default("monitoring.log_level", "info,sqlx=warn")?
            .set_default("monitoring.log_json", false)?
            .set_default("monitoring.log_target", true)?
            .set_default("monitoring.log_line_number", true)?;

        if let Some(path) = path {
            builder = builder.add_source(File::with_name(path).required(true));
        }

        // Mutable: a few sections tidy a value as they check it.
        let mut config: Self = builder
            .add_source(Environment::with_prefix("timada").separator("__"))
            .build()?
            .try_deserialize()?;

        if let Some(cluster) = &mut config.cluster {
            cluster.validate()?;
        }
        config.server.validate()?;
        config.tenants.validate()?;
        config.media.validate()?;
        config.store.validate()?;
        config.invitations.validate()?;
        if let Some(store_api) = &config.store_api {
            store_api.validate()?;
        }
        if let Some(secrets) = &config.secrets {
            secrets.validate()?;
        }
        if let Some(payments) = &config.payments {
            payments.validate(config.secrets.as_ref())?;
        }
        config.auth.validate()?;
        config.mail.validate()?;

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

impl MailConfig {
    /// Both values are parsed again when the transport is built; fail here
    /// with the key name instead.
    fn validate(&self) -> Result<(), ConfigError> {
        if let Err(err) = self.from.parse::<topcoat::mail::Mailbox>() {
            return Err(ConfigError::Message(format!(
                "mail.from: '{}' is not a mailbox ({err})",
                self.from
            )));
        }
        if let Err(err) = topcoat::mail::SmtpTransport::from_url(&self.smtp_url) {
            return Err(ConfigError::Message(format!(
                "mail.smtp_url: '{}' is not a usable address ({err})",
                self.smtp_url
            )));
        }
        Ok(())
    }
}

impl TenantsConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.max_open == Some(0) {
            return Err(ConfigError::Message(
                "tenants.max_open must be at least 1".into(),
            ));
        }
        Ok(())
    }

    /// The configured limit, or the one the machine allows.
    pub fn max_open(&self) -> usize {
        self.max_open.unwrap_or_else(crate::limits::auto_max_open)
    }
}

impl ClusterConfig {
    /// How long a node may go unheard on a tenant before the others stop
    /// waiting for it; `None` waits forever.
    pub fn absent_after(&self) -> Option<Duration> {
        (self.absent_after_secs > 0).then(|| Duration::from_secs(self.absent_after_secs))
    }

    fn validate(&mut self) -> Result<(), ConfigError> {
        if self.peers.is_empty() {
            return Err(ConfigError::Message(
                "cluster.peers must list at least one node".into(),
            ));
        }
        if self.write_backlog == 0 {
            return Err(ConfigError::Message(
                "cluster.write_backlog must be greater than 0".into(),
            ));
        }
        if self.node_id as usize >= self.peers.len() {
            return Err(ConfigError::Message(format!(
                "cluster.node_id {} is out of range for {} peers",
                self.node_id,
                self.peers.len()
            )));
        }
        if self.peers.len() == 1 {
            return Ok(());
        }
        if self.web_peers.len() != self.peers.len() {
            return Err(ConfigError::Message(format!(
                "cluster.web_peers must list the web address of each of the {} peers, in the same order",
                self.peers.len()
            )));
        }
        for url in &mut self.web_peers {
            let trimmed = url.trim_end_matches('/');
            if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
                return Err(ConfigError::Message(format!(
                    "cluster.web_peers: '{url}' must start with http:// or https://"
                )));
            }
            *url = trimmed.to_owned();
        }
        if self
            .secret
            .as_deref()
            .is_none_or(|secret| secret.len() < 32)
        {
            return Err(ConfigError::Message(
                "cluster.secret must be at least 32 characters with more than one node".into(),
            ));
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
    fn payments_seal_with_their_key_or_the_secrets_key() {
        let secrets = SecretsConfig {
            key: "a-secrets-key-of-thirty-two-chars!!".into(),
        };
        let payments = PaymentsConfig {
            secret_key: None,
            stripe_api_url: None,
        };
        assert!(payments.validate(Some(&secrets)).is_ok());
        let err = payments.validate(None).unwrap_err();
        assert!(err.to_string().contains("secrets.key"), "{err}");
        let short = PaymentsConfig {
            secret_key: Some("short".into()),
            stripe_api_url: None,
        };
        assert!(short.validate(Some(&secrets)).is_err());
        assert!(
            SecretsConfig {
                key: "short".into()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn mail_settings_are_checked_with_their_key() {
        let ok = MailConfig {
            from: "Timada <no-reply@example.com>".into(),
            smtp_url: "smtp://localhost:1025".into(),
            allow_private_hosts: false,
        };
        assert!(ok.validate().is_ok());
        let err = MailConfig {
            from: "nobody".into(),
            ..ok.clone()
        }
        .validate()
        .unwrap_err();
        assert!(err.to_string().starts_with("mail.from"), "{err}");
        let err = MailConfig {
            smtp_url: "https://example.com".into(),
            ..ok
        }
        .validate()
        .unwrap_err();
        assert!(err.to_string().starts_with("mail.smtp_url"), "{err}");
    }

    #[test]
    fn store_api_tls_files_go_together() {
        let plain = StoreApiConfig {
            listen: "127.0.0.1:4321".parse().unwrap(),
            public_addr: None,
            tls_cert: None,
            tls_key: None,
        };
        assert!(plain.validate().is_ok());
        assert!(plain.tls().is_none());
        let half = StoreApiConfig {
            tls_cert: Some("cert.pem".into()),
            ..plain.clone()
        };
        let err = half.validate().unwrap_err();
        assert!(err.to_string().contains("go together"), "{err}");
        let missing = StoreApiConfig {
            tls_cert: Some("/nonexistent/cert.pem".into()),
            tls_key: Some("/nonexistent/key.pem".into()),
            ..plain
        };
        let err = missing.validate().unwrap_err();
        assert!(err.to_string().starts_with("store_api.tls_cert"), "{err}");
    }

    #[test]
    fn store_limits_default_and_refuse_zero() {
        let limits = StoreConfig::default();
        assert_eq!(limits.shopper_codes_per_ip, 10);
        assert_eq!(limits.shopper_codes_per_key, 100);
        assert_eq!(limits.shopper_codes_window_secs, 600);
        assert_eq!(limits.contact_messages_per_ip, 5);
        assert_eq!(limits.contact_messages_per_key, 200);
        assert_eq!(limits.contact_messages_window_secs, 3600);
        assert!(limits.validate().is_ok());
        for (zero, prefix) in [
            (
                StoreConfig {
                    shopper_codes_per_ip: 0,
                    ..limits.clone()
                },
                "store.shopper_codes_",
            ),
            (
                StoreConfig {
                    shopper_codes_per_key: 0,
                    ..limits.clone()
                },
                "store.shopper_codes_",
            ),
            (
                StoreConfig {
                    shopper_codes_window_secs: 0,
                    ..limits.clone()
                },
                "store.shopper_codes_",
            ),
            (
                StoreConfig {
                    contact_messages_per_ip: 0,
                    ..limits.clone()
                },
                "store.contact_messages_",
            ),
            (
                StoreConfig {
                    contact_messages_per_key: 0,
                    ..limits.clone()
                },
                "store.contact_messages_",
            ),
            (
                StoreConfig {
                    contact_messages_window_secs: 0,
                    ..limits.clone()
                },
                "store.contact_messages_",
            ),
        ] {
            let err = zero.validate().unwrap_err();
            assert!(err.to_string().starts_with(prefix), "{err}");
        }
    }

    #[test]
    fn invitation_limits_default_and_refuse_zero() {
        let limits = InvitationsConfig::default();
        assert_eq!(limits.per_tenant, 50);
        assert_eq!(limits.per_account, 20);
        assert_eq!(limits.window_secs, 86_400);
        assert!(limits.validate().is_ok());
        for zero in [
            InvitationsConfig {
                per_tenant: 0,
                ..limits.clone()
            },
            InvitationsConfig {
                per_account: 0,
                ..limits.clone()
            },
            InvitationsConfig {
                window_secs: 0,
                ..limits.clone()
            },
        ] {
            let err = zero.validate().unwrap_err();
            assert!(err.to_string().starts_with("invitations."), "{err}");
        }
    }

    #[test]
    fn invitation_limits_refuse_at_either_cap() {
        let limits = InvitationsConfig {
            per_tenant: 3,
            per_account: 2,
            window_secs: 60,
        };
        let sent = |tenant, account| timada_tenancy::SentCounts { tenant, account };
        assert!(limits.allows(&sent(0, 0)));
        assert!(limits.allows(&sent(2, 1)));
        assert!(!limits.allows(&sent(3, 0)), "the tenant's share is used");
        assert!(!limits.allows(&sent(0, 2)), "the account's share is used");
    }

    #[test]
    fn store_limits_read_from_their_section() {
        let parsed: StoreConfig = config::Config::builder()
            .add_source(File::from_str(
                "shopper_codes_per_ip = 3\nshopper_codes_window_secs = 60\ncontact_messages_per_ip = 2\n",
                config::FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        assert_eq!(parsed.shopper_codes_per_ip, 3);
        assert_eq!(parsed.shopper_codes_per_key, 100, "unset keeps its default");
        assert_eq!(parsed.shopper_codes_window_secs, 60);
        assert_eq!(parsed.contact_messages_per_ip, 2);
        assert_eq!(
            parsed.contact_messages_per_key, 200,
            "unset keeps its default"
        );
    }

    #[test]
    fn trusted_proxies_reject_garbage_with_the_value() {
        let err = server(&["10.0.0.0/8", "traefik"]).validate().unwrap_err();
        assert!(err.to_string().contains("'traefik'"), "{err}");
    }

    fn cluster(nodes: usize, web_peers: &[&str], secret: Option<&str>) -> ClusterConfig {
        ClusterConfig {
            node_id: 0,
            peers: (0..nodes)
                .map(|i| format!("127.0.0.1:{}", 7000 + i).parse().unwrap())
                .collect(),
            listen: None,
            web_peers: web_peers.iter().map(|s| s.to_string()).collect(),
            secret: secret.map(str::to_owned),
            absent_after_secs: default_absent_after_secs(),
            write_backlog: default_write_backlog(),
        }
    }

    #[test]
    fn cluster_refuses_an_empty_write_backlog() {
        let mut config = cluster(1, &[], None);
        config.write_backlog = 0;
        assert!(config.validate().is_err());
        let mut config = cluster(1, &[], None);
        config.absent_after_secs = 0;
        assert!(config.validate().is_ok());
        assert_eq!(config.absent_after(), None);
        assert_eq!(
            cluster(1, &[], None).absent_after(),
            Some(Duration::from_secs(3600))
        );
    }

    #[test]
    fn several_nodes_need_their_web_addresses_and_a_secret() {
        const SECRET: &str = "0123456789abcdef0123456789abcdef";
        assert!(cluster(1, &[], None).validate().is_ok());

        let mut three = cluster(
            3,
            &["http://a:3000/", "http://b:3000", "https://c"],
            Some(SECRET),
        );
        assert!(three.validate().is_ok());
        assert_eq!(three.web_peers[0], "http://a:3000");

        assert!(
            cluster(3, &["http://a", "http://b"], Some(SECRET))
                .validate()
                .is_err()
        );
        assert!(
            cluster(3, &["http://a", "b:3000", "http://c"], Some(SECRET))
                .validate()
                .is_err()
        );
        assert!(
            cluster(3, &["http://a", "http://b", "http://c"], None)
                .validate()
                .is_err()
        );
        assert!(
            cluster(3, &["http://a", "http://b", "http://c"], Some("short"))
                .validate()
                .is_err()
        );
    }
}
