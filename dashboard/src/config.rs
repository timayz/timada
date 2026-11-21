//! Runtime configuration.
//!
//! Priority, highest first: `TIMADA__<SECTION>__<KEY>` environment variables,
//! the TOML file given with `--config` / `TIMADA_CONFIG`, then the defaults
//! set in [`Config::load`]. `config/default.toml` documents the full shape.

use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use config::{ConfigError, Environment, File};
use serde::Deserialize;
use timada_identity::{IdentityError, RootAccount};

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    /// Absent: a single node, talking to itself in memory. Present: one node
    /// of an Accord cluster over TCP.
    #[serde(default)]
    pub cluster: Option<ClusterConfig>,
    pub media: MediaConfig,
    pub data_keys: DataKeysConfig,
    /// Absent: this node does not expose the store API.
    #[serde(default)]
    pub store_api: Option<StoreApiConfig>,
    /// Absent: tenants cannot take payments.
    #[serde(default)]
    pub payments: Option<PaymentsConfig>,
    /// Absent: no tenant secret can be stored (shop mail servers, shopper
    /// accounts, payments).
    #[serde(default)]
    pub secrets: Option<SecretsConfig>,
    /// Invoices; every setting has a default.
    #[serde(default)]
    pub invoicing: InvoicingConfig,
    /// The shops' advertising platforms; every setting has a default.
    #[serde(default)]
    pub advertising: AdvertisingConfig,
    /// Reviews; every setting has a default.
    #[serde(default)]
    pub reviews: ReviewsConfig,
    /// The shops' newsletters; every setting has a default.
    #[serde(default)]
    pub newsletter: NewsletterConfig,
    /// The shops' automation scenarios; every setting has a default.
    #[serde(default)]
    pub automations: AutomationsConfig,
    /// What storefronts may ask for their shoppers; every setting has a
    /// default.
    #[serde(default)]
    pub store: StoreConfig,
    /// How long carts nobody touches are kept; every setting has a default.
    #[serde(default)]
    pub carts: CartsConfig,
    /// How the shops' best sellers are ranked; every setting has a default.
    #[serde(default)]
    pub best_sellers: BestSellersConfig,
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
    /// Password sign-in attempts one address (the browser's) may make within
    /// the window, right or wrong, before it is told to wait.
    #[serde(default = "default_login_attempts_per_ip")]
    pub login_attempts_per_ip: u32,
    /// Password sign-in attempts one e-mail address may be the target of
    /// within the window, from anywhere, before it is told to wait.
    #[serde(default = "default_login_attempts_per_email")]
    pub login_attempts_per_email: u32,
    /// The window both limits count over, in seconds.
    #[serde(default = "default_login_window_secs")]
    pub login_window_secs: u64,
    /// The root account: an address and the hash of a password, above every
    /// user and out of their reach. It signs in with that pair alone, so it
    /// gets in while no mail leaves; only it makes administrators.
    #[serde(deserialize_with = "root_account")]
    pub root: RootAccount,
    /// Who creates an account: anyone, or only the people root invites.
    #[serde(default, deserialize_with = "signup")]
    pub signup: Signup,
}

/// `auth.signup`: who creates an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Signup {
    /// Anyone, from the login page.
    #[default]
    Open,
    /// Only through the link root has mailed to an address.
    Invitation,
}

impl Signup {
    pub fn is_open(self) -> bool {
        self == Self::Open
    }
}

fn signup<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Signup, D::Error> {
    let value = String::deserialize(deserializer)?;
    match value.as_str() {
        "open" => Ok(Signup::Open),
        "invitation" => Ok(Signup::Invitation),
        _ => Err(serde::de::Error::custom(format!(
            "auth.signup is \"open\" or \"invitation\", not '{value}'"
        ))),
    }
}

/// `[auth.root]` as written.
#[derive(Deserialize)]
struct RootConfig {
    email: String,
    password_hash: String,
}

fn root_account<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<RootAccount, D::Error> {
    let root = RootConfig::deserialize(deserializer)?;
    RootAccount::new(&root.email, &root.password_hash).map_err(|err| {
        serde::de::Error::custom(match err {
            IdentityError::InvalidEmail => {
                format!("auth.root.email: '{}' is not an e-mail address", root.email)
            }
            _ => format!(
                "auth.root.password_hash is not the hash of a password; {ROOT_PASSWORD_HINT}"
            ),
        })
    })
}

/// How the operator makes `auth.root.password_hash`.
const ROOT_PASSWORD_HINT: &str = "make one with `timada-dashboard hash-password`";

fn default_login_attempts_per_ip() -> u32 {
    30
}

fn default_login_attempts_per_email() -> u32 {
    10
}

fn default_login_window_secs() -> u64 {
    15 * 60
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
    /// Work group this node serves, e.g. `eu-west-1`. Used as evento's default
    /// routing key: stamped on every new aggregate written here and inherited
    /// by this node's subscriptions. The read models and the per-node effects
    /// (this node's copies of pictures and keys, its store connections) call
    /// `.any_routing_key()` and see every work group; the sagas (payments,
    /// forwards, invoices, mails, releases, usage) inherit the key and run on
    /// the nodes of the work group alone, so a work group's work is done by its own
    /// nodes and not N times across the cluster; the AliExpress reads a
    /// work group's events ask for are shared out among those nodes, which tell
    /// each other the work groups they serve. One word. Absent: no routing key;
    /// the sagas follow the events written without one (every event, in a
    /// cluster that never had work groups), and the nodes without a work group
    /// share that work. A stream keeps the key it was created with: the
    /// work on the streams created before the nodes had a work group stays with
    /// the nodes without one, and with the nodes that say
    /// `also_works_for_ungrouped`.
    #[serde(default)]
    pub work_group: Option<String>,
    /// Work groups whose background work this node also takes on, e.g. while
    /// that work group's nodes are down. Each one gets a copy of every saga
    /// subscription, filtered on that work group's key (`.routing_key(r)`) with a
    /// cursor of its own (`{r}.{key}`), started where this node left that
    /// work group last: the first time, it goes through the work group's whole
    /// history for it (the sagas skip what is settled or too old). Needs
    /// `work_group`.
    #[serde(default)]
    pub also_works_for: Vec<String>,
    /// Whether this node also takes on the background work on the data
    /// created without a work group: what a cluster that ran without work groups
    /// holds from before its nodes got one (#517). It gets a copy of every
    /// saga subscription, filtered on no key (`.no_routing_key()`) under
    /// the saga's bare cursor key: the cursor this node had while it was
    /// without a work group, so it goes on from where it stopped then. What
    /// those sagas create is written in this node's work group. Needs `work_group`.
    #[serde(default)]
    pub also_works_for_ungrouped: bool,
    /// Reverse proxies allowed to report the client's address through
    /// `X-Forwarded-For`, as IP addresses or CIDR networks. Empty: the
    /// connection's peer address is the client.
    #[serde(default)]
    pub trusted_proxies: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DatabaseConfig {
    /// The database file, e.g. `data/timada.db`: every tenant's events, the
    /// snapshots, the subscription cursors and the consensus journal, and the
    /// read-model tables the pages read. Opened for several writers at once.
    pub path: PathBuf,
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
    /// How much disk a tenant's pictures may take, in bytes, unless the
    /// platform set it apart for that tenant; 0 is no quota.
    #[serde(default = "default_tenant_quota_bytes")]
    pub tenant_quota_bytes: u64,
    /// How often, in seconds, this node compares its picture files with what
    /// the store knows: orphans deleted, missing or wrong files fetched.
    #[serde(default = "default_reconcile_check_secs")]
    pub reconcile_check_secs: u64,
}

/// 5 GiB: tens of thousands of pictures in their two renditions.
fn default_tenant_quota_bytes() -> u64 {
    5 * 1024 * 1024 * 1024
}

/// Once a day.
fn default_reconcile_check_secs() -> u64 {
    24 * 60 * 60
}

impl MediaConfig {
    /// The quota every tenant has unless set apart; `None` for none.
    pub fn default_quota(&self) -> Option<u64> {
        (self.tenant_quota_bytes > 0).then_some(self.tenant_quota_bytes)
    }

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

/// How the shops' newsletter campaigns go out: every node mails its own
/// part of a campaign's subscribers, one shop's mails one after the other,
/// several shops at once.
#[derive(Debug, Deserialize, Clone)]
pub struct NewsletterConfig {
    /// How often, in seconds, this node looks for campaigns still going out;
    /// a shop's mails go out until shortly before the next look.
    #[serde(default = "default_send_check_secs")]
    pub send_check_secs: u64,
    /// The most mails one campaign sends per look from this node, however
    /// fast the shop's server takes them.
    #[serde(default = "default_mails_per_check")]
    pub mails_per_check: u32,
}

impl Default for NewsletterConfig {
    fn default() -> Self {
        Self {
            send_check_secs: default_send_check_secs(),
            mails_per_check: default_mails_per_check(),
        }
    }
}

fn default_send_check_secs() -> u64 {
    60
}

fn default_mails_per_check() -> u32 {
    1_000
}

/// How the shops' automation scenarios are run.
#[derive(Debug, Deserialize, Clone)]
pub struct AutomationsConfig {
    /// How often, in seconds, this node looks for the runs whose next step
    /// is due.
    #[serde(default = "default_automation_check_secs")]
    pub check_secs: u64,
    /// How many runs one look moves at most.
    #[serde(default = "default_automation_runs_per_check")]
    pub runs_per_check: u32,
    /// How long, in seconds, a scenario's webhook call may take.
    #[serde(default = "default_automation_webhook_timeout_secs")]
    pub webhook_timeout_secs: u64,
    /// How long, in seconds, a checkout waits for its order before it is
    /// found left unpaid.
    #[serde(default = "default_automation_abandoned_after_secs")]
    pub abandoned_after_secs: u64,
}

impl Default for AutomationsConfig {
    fn default() -> Self {
        Self {
            check_secs: default_automation_check_secs(),
            runs_per_check: default_automation_runs_per_check(),
            webhook_timeout_secs: default_automation_webhook_timeout_secs(),
            abandoned_after_secs: default_automation_abandoned_after_secs(),
        }
    }
}

fn default_automation_check_secs() -> u64 {
    15
}

fn default_automation_runs_per_check() -> u32 {
    200
}

fn default_automation_webhook_timeout_secs() -> u64 {
    10
}

fn default_automation_abandoned_after_secs() -> u64 {
    60 * 60
}

/// What storefronts may ask for their shoppers. The limits hold across the
/// cluster: a node asks the others what they let through before letting one
/// more through (`rate_limit.rs`).
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
    /// Sign-ups to the shop's news one visitor's address may hand in, at
    /// one shop, within the window (each mails a confirmation link).
    #[serde(default = "default_newsletter_signups_per_ip")]
    pub newsletter_signups_per_ip: u32,
    /// Sign-ups one API key may hand in within the window, whoever the
    /// visitors.
    #[serde(default = "default_newsletter_signups_per_key")]
    pub newsletter_signups_per_key: u32,
    /// The window both sign-up limits count over, in seconds.
    #[serde(default = "default_newsletter_signups_window_secs")]
    pub newsletter_signups_window_secs: u64,
    /// Carts one visitor's address may start, at one shop, within the
    /// window, a start refused for its item included: the limit is checked
    /// before the store is read. A shopper keeps the cart of their first
    /// add, so this holds back visitors that do not.
    #[serde(default = "default_carts_per_ip")]
    pub carts_per_ip: u32,
    /// Carts one API key may start within the window, whoever the visitors,
    /// refused starts included.
    #[serde(default = "default_carts_per_key")]
    pub carts_per_key: u32,
    /// The window both cart limits count over, in seconds.
    #[serde(default = "default_carts_window_secs")]
    pub carts_window_secs: u64,
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
            newsletter_signups_per_ip: default_newsletter_signups_per_ip(),
            newsletter_signups_per_key: default_newsletter_signups_per_key(),
            newsletter_signups_window_secs: default_newsletter_signups_window_secs(),
            carts_per_ip: default_carts_per_ip(),
            carts_per_key: default_carts_per_key(),
            carts_window_secs: default_carts_window_secs(),
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

fn default_newsletter_signups_per_ip() -> u32 {
    5
}

fn default_newsletter_signups_per_key() -> u32 {
    500
}

fn default_newsletter_signups_window_secs() -> u64 {
    60 * 60
}

fn default_carts_per_ip() -> u32 {
    30
}

fn default_carts_per_key() -> u32 {
    5_000
}

fn default_carts_window_secs() -> u64 {
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
            (
                "store.newsletter_signups_per_ip",
                u64::from(self.newsletter_signups_per_ip),
            ),
            (
                "store.newsletter_signups_per_key",
                u64::from(self.newsletter_signups_per_key),
            ),
            (
                "store.newsletter_signups_window_secs",
                self.newsletter_signups_window_secs,
            ),
            ("store.carts_per_ip", u64::from(self.carts_per_ip)),
            ("store.carts_per_key", u64::from(self.carts_per_key)),
            ("store.carts_window_secs", self.carts_window_secs),
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

/// How long carts nobody touches are kept. One that lay idle that long, with
/// nothing about it that could still move money, is dropped
/// (`idle_carts.rs`): the shopper's next visit starts another, and the
/// shop's list of open carts forgets it.
#[derive(Debug, Deserialize, Clone)]
pub struct CartsConfig {
    /// Days a cart is kept after its last change, at least 1.
    #[serde(default = "default_carts_idle_days")]
    pub idle_days: u64,
    /// How often, in seconds, this node looks for idle carts to drop.
    #[serde(default = "default_carts_check_secs")]
    pub check_secs: u64,
}

impl Default for CartsConfig {
    fn default() -> Self {
        Self {
            idle_days: default_carts_idle_days(),
            check_secs: default_carts_check_secs(),
        }
    }
}

fn default_carts_idle_days() -> u64 {
    30
}

fn default_carts_check_secs() -> u64 {
    60 * 60
}

impl CartsConfig {
    /// How long a cart is kept after its last change.
    pub fn idle(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.idle_days.saturating_mul(24 * 60 * 60))
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.idle_days == 0 {
            return Err(ConfigError::Message(
                "carts.idle_days must be greater than 0".into(),
            ));
        }
        Ok(())
    }
}

/// How the shops' best sellers are ranked (`best_sellers.rs`): what a
/// storefront's listing sorts by when asked for what sells best.
#[derive(Debug, Deserialize, Clone)]
pub struct BestSellersConfig {
    /// Days of sales a ranking counts, at least 1.
    #[serde(default = "default_best_sellers_days")]
    pub days: u64,
    /// How often, in seconds, this node ranks the shops that fall to it.
    #[serde(default = "default_best_sellers_check_secs")]
    pub check_secs: u64,
}

impl Default for BestSellersConfig {
    fn default() -> Self {
        Self {
            days: default_best_sellers_days(),
            check_secs: default_best_sellers_check_secs(),
        }
    }
}

fn default_best_sellers_days() -> u64 {
    30
}

fn default_best_sellers_check_secs() -> u64 {
    6 * 60 * 60
}

impl BestSellersConfig {
    /// How far back the sales count.
    pub fn period(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.days.saturating_mul(24 * 60 * 60))
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.days == 0 {
            return Err(ConfigError::Message(
                "best_sellers.days must be greater than 0".into(),
            ));
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

/// Products the shops import from AliExpress through the platform's own
/// app, each acting for the account of the company it sells as. The tokens
/// are kept encrypted under `secrets.key`: without it, or without the app,
/// no company can connect.
#[derive(Debug, Deserialize, Clone)]
pub struct AliexpressConfig {
    /// The platform's app on AliExpress's open platform (with dropshipping
    /// access); `/aliexpress/callback` under `server.url` is its callback.
    #[serde(default)]
    pub app_key: Option<String>,
    #[serde(default)]
    pub app_secret: Option<String>,
    /// Where AliExpress's open platform is reached; its own address when
    /// unset. Tests point it at a stand-in.
    #[serde(default)]
    pub api_url: Option<String>,
    /// Where a merchant's AliExpress account says yes to their app;
    /// AliExpress's own page when unset.
    #[serde(default)]
    pub authorize_url: Option<String>,
    /// How often, in seconds, a node renews the access about to lapse and looks
    /// for the open orders due to be read.
    #[serde(default = "default_aliexpress_check_secs")]
    pub sync_check_secs: u64,
    /// How long, in seconds, an imported product goes before its price and
    /// stock are read again, if the day's calls reach it.
    #[serde(default = "default_aliexpress_every_secs")]
    pub sync_every_secs: u64,
    /// How long, in seconds, an order placed on AliExpress goes unread before it
    /// is read again. AliExpress pushes each change of its status: this only
    /// catches a push that never came.
    #[serde(default = "default_aliexpress_order_read_every_secs")]
    pub order_read_every_secs: u64,
    /// How long, in seconds, a parcel AliExpress shipped goes before AliExpress
    /// is asked again where it is, for the order's page, while it moves; one
    /// that stays still waits up to eight periods.
    #[serde(default = "default_aliexpress_parcel_read_every_secs")]
    pub parcel_read_every_secs: u64,
    /// How many calls a day the app may make, over every shop and node: read
    /// the app's quota on AliExpress's open platform and set this from it. A
    /// sale always reads what it sells; the nodes spend what is left on the
    /// other products, the open orders and the parcels, each from a lane of
    /// its own. 0: nothing but what sales and members ask for.
    #[serde(default = "default_aliexpress_calls_per_day")]
    pub calls_per_day: u64,
    /// How recent, in seconds, a read of an imported product must be for a
    /// payment to go by it instead of reading the product again.
    #[serde(default = "default_aliexpress_read_fresh_secs")]
    pub read_fresh_secs: u64,
    /// What AliExpress adds to an order for each kind of goods it ships into
    /// the EU from outside, in euro cents: the flat customs duty of a parcel
    /// worth 150 EUR or less (the destination's VAT goes on top), and the
    /// processing fee of any parcel. Imports are priced with them; both are
    /// dated in law, so the operator follows the law here. 0: none.
    #[serde(default = "default_aliexpress_import_duty_cents")]
    pub import_duty_cents: u64,
    #[serde(default = "default_aliexpress_import_fee_cents")]
    pub import_fee_cents: u64,
    /// How many orders a company's AliExpress account places an hour at
    /// most, over every shop it sells for, spread across the hour; the others
    /// wait their turn. AliExpress says nothing of how many orders a buyer
    /// account may place before its risk control looks at it: many retail
    /// orders an hour to many addresses from one account is the pattern it
    /// may flag. 0: as they come.
    #[serde(default)]
    pub orders_per_hour: u32,
}

fn default_aliexpress_check_secs() -> u64 {
    60 * 60
}

fn default_aliexpress_every_secs() -> u64 {
    24 * 60 * 60
}

fn default_aliexpress_order_read_every_secs() -> u64 {
    24 * 60 * 60
}

fn default_aliexpress_parcel_read_every_secs() -> u64 {
    6 * 60 * 60
}

fn default_aliexpress_calls_per_day() -> u64 {
    12_000
}

fn default_aliexpress_read_fresh_secs() -> u64 {
    60
}

/// 3 EUR per kind of goods, from 2026-07-01 until 2028-07-01.
fn default_aliexpress_import_duty_cents() -> u64 {
    300
}

/// 2 EUR per kind of goods, from 2026-11-01.
fn default_aliexpress_import_fee_cents() -> u64 {
    200
}

impl Default for AliexpressConfig {
    fn default() -> Self {
        Self {
            app_key: None,
            app_secret: None,
            api_url: None,
            authorize_url: None,
            sync_check_secs: default_aliexpress_check_secs(),
            sync_every_secs: default_aliexpress_every_secs(),
            order_read_every_secs: default_aliexpress_order_read_every_secs(),
            parcel_read_every_secs: default_aliexpress_parcel_read_every_secs(),
            calls_per_day: default_aliexpress_calls_per_day(),
            read_fresh_secs: default_aliexpress_read_fresh_secs(),
            import_duty_cents: default_aliexpress_import_duty_cents(),
            import_fee_cents: default_aliexpress_import_fee_cents(),
            orders_per_hour: 0,
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
    /// Where the European Central Bank's daily euro reference rates are
    /// read, for sales priced in another currency; the Bank's own address
    /// when unset. Tests point it at a stand-in.
    #[serde(default)]
    pub euro_rates_url: Option<String>,
}

/// The shops' advertising platforms.
#[derive(Debug, Deserialize, Clone, Default)]
pub struct AdvertisingConfig {
    /// Where the sales of the shops that gave a Conversions API token are
    /// sent to Meta; Meta's own address when unset. Tests point it at a
    /// stand-in.
    #[serde(default)]
    pub meta_api_url: Option<String>,
}

/// Card payments: each tenant connects its own Stripe account; this node
/// keeps the account's secrets encrypted under `secrets.key`.
#[derive(Debug, Deserialize, Clone)]
pub struct PaymentsConfig {
    /// Where Stripe's API is reached; Stripe's own address when unset. Tests
    /// point it at a local stand-in.
    #[serde(default)]
    pub stripe_api_url: Option<String>,
    /// How often, in seconds, this node looks for card forms left open past
    /// the time their goods were held for, and closes them with Stripe; and
    /// for orders whose money Stripe was asked to take or give back without
    /// answering, and settles them.
    #[serde(default = "default_cancel_check_secs")]
    pub cancel_check_secs: u64,
}

fn default_cancel_check_secs() -> u64 {
    60
}

impl PaymentsConfig {
    fn validate(&self, secrets: Option<&SecretsConfig>) -> Result<(), ConfigError> {
        if secrets.is_none() {
            return Err(ConfigError::Message(
                "payments needs secrets.key (set TIMADA__SECRETS__KEY)".into(),
            ));
        }
        Ok(())
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
    /// Each is `ip:port` or `host:port`; a name is looked up each time the node
    /// is dialled, so a node that comes back at another address is found again.
    pub peers: Vec<evento::cluster::PeerAddr>,
    /// How many of `peers`, the first ones, the cluster was founded with;
    /// absent, all of them. A node listed after them is no member until an
    /// administrator adds it on the cluster page (`Cluster::add_node`):
    /// started over an empty store, it waits for that, then copies the store
    /// from a member, and founds nothing. The same on every node, for as
    /// long as the cluster lives: one founded with three nodes says 3
    /// however many it has now.
    #[serde(default)]
    pub founders: Option<u64>,
    /// Address to bind for inbound Accord traffic. Defaults to `peers[node_id]`
    /// when that is an address, and is required when it is a name; set it to
    /// `0.0.0.0:<port>` in a container.
    #[serde(default)]
    pub listen: Option<SocketAddr>,
    /// Every node's web address as the other nodes reach it, in `peers`
    /// order, e.g. `https://10.0.0.2:7001`: the port each keeps for the
    /// other nodes (`web_listen`), not `server.port`, over the TLS of
    /// `tls`. Product pictures and shopper keys are copied between nodes
    /// through it, and the rate limits ask the other nodes through it.
    /// Required with more than one node.
    #[serde(default)]
    pub web_peers: Vec<String>,
    /// Address to bind for what the other nodes ask of this one: the port of
    /// its entry in `web_peers`, apart from `server.port`, which serves none
    /// of it. Defaults to that entry when it gives an address and a port,
    /// and is required when it is a name; set it to `0.0.0.0:<port>` in a
    /// container.
    #[serde(default)]
    pub web_listen: Option<SocketAddr>,
    /// Mutual TLS between the nodes, on both of their ports: the authority
    /// every node's certificate is issued by, this node's certificate and
    /// its key (`ca`, `cert`, `key`: PEM files, as a cert-manager secret
    /// holds them). A node is let in as the node its certificate names: it
    /// must be valid for the host, or the address, the node's entry in
    /// `peers` gives, and in `web_peers`. The files are read again every few
    /// seconds, so a certificate is renewed by replacing them. Required with
    /// more than one node.
    #[serde(default)]
    pub tls: Option<evento::cluster::TlsSettings>,
    /// A node not heard from for this long is no longer waited for: the
    /// others tidy up finished writes without it. When it returns it is told
    /// so, and comes back by copying the store from a peer (`rejoin`, then
    /// restart). A day by default: the copy is the costly way back for a
    /// large store, and `write_backlog` holds a day's writes many times
    /// over, so a node down for a night is simply caught up. 0 never gives
    /// a node up.
    #[serde(default = "default_absent_after_secs")]
    pub absent_after_secs: u64,
    /// Finished writes this node keeps for a node that is away. Past it the
    /// node refuses new writes until the node returns or is given up.
    #[serde(default = "default_write_backlog")]
    pub write_backlog: usize,
}

fn default_absent_after_secs() -> u64 {
    24 * 3600
}

fn default_write_backlog() -> usize {
    100_000
}

#[derive(Debug, Deserialize, Clone)]
pub struct MonitoringConfig {
    /// A `tracing_subscriber::EnvFilter` directive, e.g. `info,hyper=warn`. The
    /// default quiets the search index and the store engine, which report
    /// routine work at `info`.
    pub log_level: String,
    pub log_json: bool,
    pub log_target: bool,
    pub log_line_number: bool,
    /// The most a read model may trail the store, in seconds, for `/ready`
    /// to answer 200: a node back from a night away, or one that just
    /// copied the store, answers 503 until its read models caught up, and a
    /// balancer polling `/ready` keeps traffic off it meanwhile. 0 reports
    /// the lag and never judges it.
    #[serde(default = "default_ready_lag_secs")]
    pub ready_lag_secs: u64,
    /// Where to serve `/metrics` (Prometheus text) and `/ready`, e.g.
    /// `127.0.0.1:9100`: an internal address, as the figures name tenants.
    /// Absent: no metrics served; `/ready` is on the node's own port either
    /// way.
    #[serde(default)]
    pub metrics_listen: Option<SocketAddr>,
}

fn default_ready_lag_secs() -> u64 {
    300
}

impl MonitoringConfig {
    /// The most a read model may trail the store for the node to be ready;
    /// `None` never judges it.
    pub fn ready_lag(&self) -> Option<std::time::Duration> {
        (self.ready_lag_secs > 0).then(|| std::time::Duration::from_secs(self.ready_lag_secs))
    }
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Self, ConfigError> {
        let mut builder = config::Config::builder()
            .set_default("server.url", "https://timada.localhost")?
            // Traefik reaches the dev server through host.docker.internal, which
            // is not the loopback interface.
            .set_default("server.host", "0.0.0.0")?
            .set_default("server.port", 3000)?
            .set_default("database.path", "data/timada.db")?
            .set_default("media.dir", "data/media")?
            .set_default("media.public_url", "https://timada.localhost/media")?
            .set_default("data_keys.dir", "data/shopper-keys")?
            .set_default("auth.session_lifetime_secs", 14 * 24 * 60 * 60)?
            .set_default("auth.rp_name", "Timada")?
            .set_default("mail.from", "Timada <no-reply@timada.localhost>")?
            .set_default("mail.smtp_url", "smtp://localhost:1025")?
            .set_default(
                "monitoring.log_level",
                "info,tantivy=warn,turso_core::mvcc=warn",
            )?
            .set_default("monitoring.log_json", false)?
            .set_default("monitoring.log_target", true)?
            .set_default("monitoring.log_line_number", true)?;

        if let Some(path) = path {
            builder = builder.add_source(File::with_name(path).required(true));
        }

        let built = builder
            .add_source(Environment::with_prefix("timada").separator("__"))
            .build()?;
        // Said here, in the operator's words, rather than as a missing field.
        for key in ["auth.root.email", "auth.root.password_hash"] {
            if built.get_string(key).is_err() {
                return Err(ConfigError::Message(format!(
                    "{key} is not set: the root account is defined in the configuration \
                     (auth.root.email, and auth.root.password_hash, \
                     TIMADA__AUTH__ROOT__PASSWORD_HASH in production; {ROOT_PASSWORD_HINT})"
                )));
            }
        }
        // Mutable: a few sections tidy a value as they check it.
        let mut config: Self = built.try_deserialize()?;

        if let Some(cluster) = &mut config.cluster {
            cluster.validate()?;
        }
        config.server.validate()?;
        config.media.validate()?;
        config.store.validate()?;
        config.carts.validate()?;
        config.best_sellers.validate()?;
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
        if let Some(work_group) = &self.work_group
            && (work_group.is_empty() || work_group.contains(char::is_whitespace))
        {
            return Err(ConfigError::Message(format!(
                "server.work_group: '{work_group}' is empty or has spaces in it; a work group is one word"
            )));
        }
        if !self.also_works_for.is_empty() && self.work_group.is_none() {
            return Err(ConfigError::Message(
                "server.also_works_for needs server.work_group: a node takes on other work groups' work besides its own work group's".into(),
            ));
        }
        if self.also_works_for_ungrouped && self.work_group.is_none() {
            return Err(ConfigError::Message(
                "server.also_works_for_ungrouped needs server.work_group: a node without a work group already does the work on the data created without one".into(),
            ));
        }
        for (i, served) in self.also_works_for.iter().enumerate() {
            if served.is_empty() || served.contains(char::is_whitespace) {
                return Err(ConfigError::Message(format!(
                    "server.also_works_for: '{served}' is empty or has spaces in it; a work group is one word"
                )));
            }
            if self.work_group.as_deref() == Some(served.as_str()) {
                return Err(ConfigError::Message(format!(
                    "server.also_works_for: '{served}' is this node's own work group"
                )));
            }
            if self.also_works_for[..i].contains(served) {
                return Err(ConfigError::Message(format!(
                    "server.also_works_for: '{served}' is listed twice"
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
        for (name, value) in [
            (
                "auth.login_attempts_per_ip",
                u64::from(self.login_attempts_per_ip),
            ),
            (
                "auth.login_attempts_per_email",
                u64::from(self.login_attempts_per_email),
            ),
            ("auth.login_window_secs", self.login_window_secs),
        ] {
            if value == 0 {
                return Err(ConfigError::Message(format!(
                    "{name} must be greater than 0"
                )));
            }
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

impl ClusterConfig {
    /// The node's settings, as evento's host takes them: everything here but
    /// the web addresses, which are this application's own channel between
    /// the nodes.
    pub fn settings(&self) -> evento::cluster::Settings {
        evento::cluster::Settings {
            node_id: self.node_id,
            peers: self.peers.clone(),
            founders: self.founders,
            listen: self.listen,
            absent_after_secs: self.absent_after_secs,
            write_backlog: self.write_backlog,
            linearizable_reads: None,
            tls: self.tls.clone(),
        }
    }

    /// Where this node takes the other nodes' requests: `web_listen`, or the
    /// address and port of its own entry in `web_peers`; `None` when neither
    /// gives one.
    pub fn web_listen_addr(&self) -> Option<SocketAddr> {
        self.web_listen.or_else(|| {
            let own = self.web_peers.get(usize::try_from(self.node_id).ok()?)?;
            let rest = own
                .strip_prefix("http://")
                .or_else(|| own.strip_prefix("https://"))?;
            rest.split('/').next()?.parse().ok()
        })
    }

    fn validate(&mut self) -> Result<(), ConfigError> {
        // The node's own checks (peers, node_id, write_backlog) are evento's.
        self.settings()
            .validate()
            .map_err(|err| ConfigError::Message(err.to_string()))?;
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
            if !trimmed.starts_with("https://") {
                return Err(ConfigError::Message(format!(
                    "cluster.web_peers: '{url}' must start with https://: the nodes talk over TLS"
                )));
            }
            *url = trimmed.to_owned();
        }
        if self.web_listen_addr().is_none() {
            return Err(ConfigError::Message(format!(
                "cluster.web_listen is required: this node's entry in cluster.web_peers, '{}', gives no address to bind (0.0.0.0:<port> in a container)",
                self.web_peers[self.node_id as usize]
            )));
        }
        let Some(tls) = &self.tls else {
            return Err(ConfigError::Message(
                "cluster.tls is required with more than one node: the nodes let each other in by their certificates (ca, cert and key)".into(),
            ));
        };
        for (name, path) in [
            ("cluster.tls.ca", &tls.ca),
            ("cluster.tls.cert", &tls.cert),
            ("cluster.tls.key", &tls.key),
        ] {
            if !path.is_file() {
                return Err(ConfigError::Message(format!(
                    "{name}: '{}' is not a readable file",
                    path.display()
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn server(trusted_proxies: &[&str]) -> ServerConfig {
        ServerConfig {
            url: "https://timada.localhost".into(),
            host: "0.0.0.0".into(),
            port: 3000,
            work_group: None,
            also_works_for: Vec::new(),
            also_works_for_ungrouped: false,
            trusted_proxies: trusted_proxies.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn work_groups(work_group: Option<&str>, also_works_for: &[&str]) -> ServerConfig {
        ServerConfig {
            work_group: work_group.map(str::to_owned),
            also_works_for: also_works_for.iter().map(|s| s.to_string()).collect(),
            ..server(&[])
        }
    }

    #[test]
    fn also_served_work_groups_need_a_work_group_and_name_other_ones_once() {
        assert!(work_groups(None, &[]).validate().is_ok());
        assert!(work_groups(Some("eu"), &[]).validate().is_ok());
        assert!(work_groups(Some("eu"), &["us", "ap"]).validate().is_ok());
        let err = work_groups(None, &["us"]).validate().unwrap_err();
        assert!(err.to_string().contains("server.work_group"), "{err}");
        let err = work_groups(Some("eu"), &["eu"]).validate().unwrap_err();
        assert!(err.to_string().contains("own work group"), "{err}");
        let err = work_groups(Some("eu"), &["us", "us"])
            .validate()
            .unwrap_err();
        assert!(err.to_string().contains("twice"), "{err}");
        assert!(work_groups(Some("eu"), &[" "]).validate().is_err());
        assert!(work_groups(Some("eu"), &[" us"]).validate().is_err());
        // A work group is one word: it names a routing key and travels in the
        // node's gossip with the others it serves.
        let err = work_groups(Some("eu west"), &[]).validate().unwrap_err();
        assert!(err.to_string().contains("one word"), "{err}");
        assert!(work_groups(Some(""), &[]).validate().is_err());
        assert!(work_groups(Some("eu"), &["us east"]).validate().is_err());
    }

    #[test]
    fn serving_the_data_without_a_work_group_needs_a_work_group() {
        let ungrouped = |work_group: Option<&str>| ServerConfig {
            also_works_for_ungrouped: true,
            ..work_groups(work_group, &[])
        };
        assert!(ungrouped(Some("eu")).validate().is_ok());
        let err = ungrouped(None).validate().unwrap_err();
        assert!(err.to_string().contains("server.work_group"), "{err}");
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
    fn payments_need_the_secrets_key() {
        let secrets = SecretsConfig {
            key: "a-secrets-key-of-thirty-two-chars!!".into(),
        };
        let payments = PaymentsConfig {
            stripe_api_url: None,
            cancel_check_secs: default_cancel_check_secs(),
        };
        assert!(payments.validate(Some(&secrets)).is_ok());
        let err = payments.validate(None).unwrap_err();
        assert!(err.to_string().contains("secrets.key"), "{err}");
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

    fn cluster(
        nodes: usize,
        web_peers: &[&str],
        tls: Option<evento::cluster::TlsSettings>,
    ) -> ClusterConfig {
        ClusterConfig {
            node_id: 0,
            founders: None,
            peers: (0..nodes)
                .map(|i| format!("127.0.0.1:{}", 7000 + i).parse().unwrap())
                .collect(),
            listen: None,
            web_peers: web_peers.iter().map(|s| s.to_string()).collect(),
            web_listen: None,
            tls,
            absent_after_secs: default_absent_after_secs(),
            write_backlog: default_write_backlog(),
        }
    }

    /// Three files where `[cluster.tls]` looks for them; what they hold is
    /// read when the node opens, not here.
    fn tls_files() -> evento::cluster::TlsSettings {
        let dir = std::env::temp_dir().join(format!("timada-tls-{}", ulid::Ulid::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let [ca, cert, key] = ["ca.crt", "tls.crt", "tls.key"].map(|name| {
            let path = dir.join(name);
            std::fs::write(&path, "").unwrap();
            path
        });
        evento::cluster::TlsSettings { ca, cert, key }
    }

    #[test]
    fn cluster_refuses_an_empty_write_backlog() {
        let mut config = cluster(1, &[], None);
        config.write_backlog = 0;
        assert!(config.validate().is_err());
        let mut config = cluster(1, &[], None);
        config.absent_after_secs = 0;
        assert!(config.validate().is_ok());
        assert_eq!(config.settings().absent_after(), None);
        assert_eq!(
            cluster(1, &[], None).settings().absent_after(),
            Some(Duration::from_secs(24 * 3600))
        );
    }

    #[test]
    fn a_node_named_by_host_needs_its_listen_address() {
        let mut config: ClusterConfig = config::Config::builder()
            .add_source(File::from_str(
                "node_id = 0\npeers = [\"timada-0.timada-headless:7000\"]\n",
                config::FileFormat::Toml,
            ))
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("cluster.listen"), "{err}");

        config.listen = Some("0.0.0.0:7000".parse().unwrap());
        assert!(config.validate().is_ok());
    }

    /// What the helm chart counts on: every pod reads one file, which names
    /// the nodes and not which of them the pod is, and the pod's ordinal
    /// reaches it as a variable (a string, and numbers written by helm are
    /// floats).
    #[test]
    fn a_node_takes_its_id_from_the_environment() {
        let env = Environment::with_prefix("timada")
            .separator("__")
            .source(Some(
                [("TIMADA__CLUSTER__NODE_ID".to_owned(), "2".to_owned())].into(),
            ));
        #[derive(Deserialize)]
        struct OnlyCluster {
            cluster: ClusterConfig,
        }
        let mut config: OnlyCluster = config::Config::builder()
            .add_source(File::from_str(
                "[cluster]\nlisten = \"0.0.0.0:7000\"\nweb_listen = \"0.0.0.0:7001\"\nwrite_backlog = 5000.0\n\
                 peers = [\"timada-0.timada-headless:7000\", \"timada-1.timada-headless:7000\", \"timada-2.timada-headless:7000\"]\n\
                 web_peers = [\"https://timada-0.timada-headless:7001\", \"https://timada-1.timada-headless:7001\", \"https://timada-2.timada-headless:7001\"]\n",
                config::FileFormat::Toml,
            ))
            .add_source(env)
            .build()
            .unwrap()
            .try_deserialize()
            .unwrap();
        config.cluster.tls = Some(tls_files());
        assert!(config.cluster.validate().is_ok());
        assert_eq!(config.cluster.node_id, 2);
        assert_eq!(config.cluster.write_backlog, 5000);
    }

    #[test]
    fn several_nodes_need_their_web_addresses_and_their_certificates() {
        // Alone, a node talks to nobody.
        assert!(cluster(1, &[], None).validate().is_ok());

        let mut three = cluster(
            3,
            &["https://a:7001/", "https://b:7001", "https://c"],
            Some(tls_files()),
        );
        three.web_listen = Some("0.0.0.0:7001".parse().unwrap());
        assert!(three.validate().is_ok());
        assert_eq!(three.web_peers[0], "https://a:7001");
        assert_eq!(three.settings().tls, three.tls);

        let refused = |mut config: ClusterConfig, says: &str| {
            config.web_listen = Some("0.0.0.0:7001".parse().unwrap());
            let err = config.validate().unwrap_err();
            assert!(err.to_string().contains(says), "{says}: {err}");
        };
        refused(
            cluster(3, &["https://a", "https://b"], Some(tls_files())),
            "each of the 3 peers",
        );
        refused(
            cluster(3, &["https://a", "b:7001", "https://c"], Some(tls_files())),
            "https://",
        );
        // Never in clear between the nodes.
        refused(
            cluster(3, &["http://a", "http://b", "http://c"], Some(tls_files())),
            "https://",
        );
        refused(
            cluster(3, &["https://a", "https://b", "https://c"], None),
            "cluster.tls is required",
        );
        let mut missing = tls_files();
        missing.key = missing.key.with_file_name("gone.key");
        refused(
            cluster(3, &["https://a", "https://b", "https://c"], Some(missing)),
            "cluster.tls.key",
        );
    }

    #[test]
    fn a_node_takes_the_other_nodes_requests_at_its_own_web_address() {
        // An address and a port: bound as it is.
        let mut by_address = cluster(
            3,
            &[
                "https://127.0.0.1:7100/",
                "https://127.0.0.1:7101",
                "https://127.0.0.1:7102",
            ],
            Some(tls_files()),
        );
        by_address.node_id = 1;
        assert!(by_address.validate().is_ok());
        assert_eq!(
            by_address.web_listen_addr(),
            Some("127.0.0.1:7101".parse().unwrap())
        );

        // A name: the address to bind is given.
        let mut by_name = cluster(
            3,
            &["https://a:7001", "https://b:7001", "https://c:7001"],
            Some(tls_files()),
        );
        let err = by_name.validate().unwrap_err();
        assert!(err.to_string().contains("cluster.web_listen"), "{err}");
        by_name.web_listen = Some("0.0.0.0:7001".parse().unwrap());
        assert!(by_name.validate().is_ok());
        assert_eq!(
            by_name.web_listen_addr(),
            Some("0.0.0.0:7001".parse().unwrap())
        );
    }

    #[test]
    fn the_root_account_is_read_and_checked() {
        const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$diXOV+PTSdOCfHj2wDAlMA$iVEeQUJsww/h81mHmrcmcJRWuZs2mY3bopNrchI/yog";
        #[derive(Debug, Deserialize)]
        struct Auth {
            #[serde(deserialize_with = "root_account")]
            root: RootAccount,
        }
        let read = |email: &str, password_hash: &str| {
            let toml = format!("[root]\nemail = '{email}'\npassword_hash = '{password_hash}'\n");
            config::Config::builder()
                .add_source(File::from_str(&toml, config::FileFormat::Toml))
                .build()
                .unwrap()
                .try_deserialize::<Auth>()
                .map(|auth| auth.root)
        };

        let root = read("Root@Example.com", HASH).unwrap();
        assert_eq!(root.email(), "root@example.com");

        let err = read("root", HASH).unwrap_err();
        assert!(err.to_string().contains("auth.root.email"), "{err}");
        // A password typed where its hash goes is told how to make one, and
        // not repeated.
        let err = read("root@example.com", "correct horse battery").unwrap_err();
        assert!(err.to_string().contains("hash-password"), "{err}");
        assert!(!err.to_string().contains("correct horse"), "{err}");
    }

    #[test]
    fn sign_up_is_open_unless_the_configuration_closes_it() {
        #[derive(Debug, Deserialize)]
        struct Auth {
            #[serde(default, deserialize_with = "signup")]
            signup: Signup,
        }
        let read = |toml: &str| {
            config::Config::builder()
                .add_source(File::from_str(toml, config::FileFormat::Toml))
                .build()
                .unwrap()
                .try_deserialize::<Auth>()
                .map(|auth| auth.signup)
        };

        assert_eq!(read("").unwrap(), Signup::Open);
        assert_eq!(read("signup = 'open'").unwrap(), Signup::Open);
        assert_eq!(read("signup = 'invitation'").unwrap(), Signup::Invitation);
        assert!(!Signup::Invitation.is_open());
        let err = read("signup = 'closed'").unwrap_err();
        assert!(
            err.to_string()
                .contains("auth.signup is \"open\" or \"invitation\", not 'closed'"),
            "{err}"
        );
    }

    #[test]
    fn the_browser_suite_configuration_loads() {
        let path = format!("{}/../config/e2e.toml", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.auth.root.email(), "root-e2e@example.com");
    }
}
