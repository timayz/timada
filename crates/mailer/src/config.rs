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
    /// Where the shop itself is written to about what needs somebody — a
    /// payment being disputed, with its deadline. `None`: nobody is told.
    pub alerts_to: Option<String>,
    /// Events older than this many seconds are not e-mailed about. A
    /// subscription replays history when it first starts (or after a long
    /// outage): without this guard, plugging the mailer into an existing shop
    /// would write to every past customer.
    pub max_event_age_secs: u64,
    /// Where the customer an e-mail is written to reads their order, when it
    /// is not their account: the path of a guest's signed link. **Hosts leave
    /// it `None`** — the mailer fills it, e-mail by e-mail, from
    /// [`GuestOrderLinks`] when the customer ordered without an account.
    pub guest_order_path: Option<String>,
}

/// Where a guest reads an order: the path (with its key) for an order id.
/// Optional subscription data — `.data(GuestOrderLinks::new(|order_id| …))`.
/// Without it, a guest's e-mails point to the account pages they cannot open.
#[derive(Clone)]
pub struct GuestOrderLinks(std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>);

impl GuestOrderLinks {
    pub fn new(path_of: impl Fn(&str) -> String + Send + Sync + 'static) -> Self {
        Self(std::sync::Arc::new(path_of))
    }

    pub fn path(&self, order_id: &str) -> String {
        (self.0)(order_id)
    }
}

impl MailerConfig {
    /// One day: late enough to survive a restart, early enough to stay relevant.
    pub const DEFAULT_MAX_EVENT_AGE_SECS: u64 = 86_400;

    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }

    /// Where the customer this e-mail is written to reads the order: their
    /// account, or a guest's signed link. For a host's own [`crate::Templates`].
    /// Where they follow a return: its slip in their account, or — a guest —
    /// the signed link of the order, whose page lists its returns.
    pub fn return_url(&self, return_id: &str) -> String {
        match &self.guest_order_path {
            Some(path) => self.url(path),
            None => self.url(&format!("/account/returns/{return_id}")),
        }
    }

    pub fn order_url(&self, order_id: &str) -> String {
        match &self.guest_order_path {
            Some(path) => self.url(path),
            None => self.url(&format!("/account/orders/{order_id}")),
        }
    }
}
