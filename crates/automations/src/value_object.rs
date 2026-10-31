use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// Who writes what the host does on its own behalf (a run it advances, a
/// code it creates), in the `*_by` fields.
pub const AUTOMATION_ACTOR: &str = "timada-automations";

/// The longest name of a scenario, in characters.
pub const MAX_NAME_LEN: usize = 80;
/// The most steps a scenario holds, every lane counted.
pub const MAX_STEPS: usize = 50;
/// How deep branching steps nest: a lane inside this many branches holds no
/// branching step.
pub const MAX_DEPTH: usize = 4;
/// The largest definition, as JSON, in bytes.
pub const MAX_DEFINITION_BYTES: usize = 256 * 1024;
/// The most rules one condition checks.
pub const MAX_RULES: usize = 10;
/// The longest wait, in seconds.
pub const MAX_WAIT_SECS: u64 = 90 * 24 * 60 * 60;
/// The longest subject of a notice, in characters.
pub const MAX_SUBJECT_LEN: usize = 200;
/// The longest note of a notice, in characters.
pub const MAX_NOTE_LEN: usize = 2000;
/// The longest text a condition compares with, in characters.
pub const MAX_VALUE_LEN: usize = 200;

/// The definition format this build reads and writes. A definition of a
/// later format (written by a newer node) is shown, never edited.
pub const DEFINITION_SCHEMA: u32 = 1;

/// Where a scenario stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScenarioStatus {
    /// Never published: nothing runs.
    #[default]
    Draft,
    /// Published: what triggers it starts a run.
    Active,
    /// Published, then held.
    Paused,
    /// Deleted from the merchant's view.
    Archived,
}

impl ScenarioStatus {
    pub const ALL: [Self; 4] = [Self::Draft, Self::Active, Self::Paused, Self::Archived];

    /// The value stored in the read model.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }
}

/// A fresh id for a scenario: a ULID, sortable by when it was made.
pub fn new_scenario_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// A name trimmed, or `None` when it is empty, too long or holds control
/// characters.
pub fn checked_name(name: &str) -> Option<String> {
    let name = name.trim();
    (!name.is_empty()
        && name.chars().count() <= MAX_NAME_LEN
        && !name.chars().any(char::is_control))
    .then(|| name.to_owned())
}

/// How long a step taken to act may go without saying how it went before
/// another node concludes the one acting stopped, in seconds. Longer than
/// any mail or call a step makes.
pub const CLAIM_LEASE_SECS: u64 = 10 * 60;
/// How long a run is listed once it ended, in seconds.
pub const RUN_HISTORY_SECS: u64 = 90 * 24 * 60 * 60;

/// Where a run stands.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RunStatus {
    /// Going through its steps.
    #[default]
    Running,
    /// In a wait.
    Waiting,
    Finished,
    /// A step failed for good.
    Failed,
    Cancelled,
}

impl RunStatus {
    pub const ALL: [Self; 5] = [
        Self::Running,
        Self::Waiting,
        Self::Finished,
        Self::Failed,
        Self::Cancelled,
    ];

    /// The value stored in the read model.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|status| status.as_str() == value)
    }

    /// Whether it ended.
    pub fn is_done(self) -> bool {
        matches!(self, Self::Finished | Self::Failed | Self::Cancelled)
    }
}

/// The id of the run an event starts of a scenario
/// ([`Run`](crate::aggregator::run::Run)).
pub fn run_id(scenario_id: &str, trigger_event_id: &str) -> String {
    evento::hash_ids(vec!["automation-run", scenario_id, trigger_event_id])
}

/// Where a scenario stands for what starts runs and moves them on, as the
/// trigger subscription last saw it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenerState {
    /// Published and running: its trigger starts runs.
    Active,
    /// Held: nothing starts, runs under way wait.
    Paused,
    /// Deleted: nothing starts, runs under way are cancelled.
    Archived,
}

impl ListenerState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Archived => "archived",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::Active, Self::Paused, Self::Archived]
            .into_iter()
            .find(|state| state.as_str() == value)
    }
}

/// The longest address, in characters.
pub use timada_core::MAX_EMAIL_LEN;

/// An address as the lists keep it: the one canonical form every context
/// uses ([`timada_core::normalize_email`]), so an address that placed an
/// order is the address on the opt-out list.
pub use timada_core::normalize_email;

/// The id of an address on the shop's opt-out list
/// ([`OptOut`](crate::aggregator::opt_out::OptOut)), of the normalized
/// address.
pub fn opt_out_id(normalized_email: &str) -> String {
    evento::hash_ids(vec!["automation-opt-out", normalized_email])
}

/// The id of one scenario's mail to one address on one day (days since the
/// epoch).
pub fn mail_claim_id(scenario_id: &str, normalized_email: &str, day: u64) -> String {
    let day = day.to_string();
    evento::hash_ids(vec!["automation-mail", scenario_id, normalized_email, &day])
}

type HmacSha256 = Hmac<Sha256>;

/// The MAC binding an opt-out id to its tenant, under `key`.
fn opt_out_mac(key: &[u8; 32], tenant_id: &str, opt_out_id: &str) -> HmacSha256 {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC takes any key length");
    for part in ["unsubscribe", tenant_id, opt_out_id] {
        mac.update(&(part.len() as u64).to_be_bytes());
        mac.update(part.as_bytes());
    }
    mac
}

/// The token of the link that puts `normalized_email` on the tenant's
/// opt-out list: `{opt_out_id}.{tag}`. It holds no address, needs nothing
/// stored to check, and never expires. `key` is the host's, the same on
/// every node.
pub fn opt_out_token(key: &[u8; 32], tenant_id: &str, normalized_email: &str) -> String {
    let id = opt_out_id(normalized_email);
    let tag = opt_out_mac(key, tenant_id, &id).finalize().into_bytes();
    let tag = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(tag);
    format!("{id}.{tag}")
}

/// The opt-out id a token names, when it checks out for the tenant.
pub fn opt_out_id_of_token(key: &[u8; 32], tenant_id: &str, token: &str) -> Option<String> {
    let (id, tag) = token.trim().rsplit_once('.')?;
    let tag = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(tag)
        .ok()?;
    opt_out_mac(key, tenant_id, id).verify_slice(&tag).ok()?;
    Some(id.to_owned())
}

/// The id of a checkout found left
/// ([`AbandonedCheckout`](crate::aggregator::abandoned::AbandonedCheckout)).
pub fn abandoned_checkout_id(cart_id: &str, request_id: &str) -> String {
    evento::hash_ids(vec!["abandoned-checkout", cart_id, request_id])
}
