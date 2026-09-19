/// What the e-mails need to know about the shop. Passed to the mailer
/// subscription with `.data(config)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailerConfig {
    /// The `From` mailbox, e.g. `Timada <no-reply@timada.example>`.
    pub from: String,
    /// How the shop signs its e-mails.
    pub shop_name: String,
    /// Absolute storefront URL without a trailing slash, for links.
    pub base_url: String,
    /// Where customers send their returns, one line per address line; printed
    /// in the "return approved" e-mail.
    pub returns_address: String,
    /// Events older than this many seconds are not e-mailed about. A
    /// subscription replays history when it first starts (or after a long
    /// outage): without this guard, plugging the mailer into an existing shop
    /// would write to every past customer.
    pub max_event_age_secs: u64,
}

impl MailerConfig {
    /// One day: late enough to survive a restart, early enough to stay relevant.
    pub const DEFAULT_MAX_EVENT_AGE_SECS: u64 = 86_400;

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }
}
