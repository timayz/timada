//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// A shop's connection to a Google account. Its id is made of the shop's
/// ([`connection_id`](crate::connection_id)), and it lives in the platform's
/// tenant, out of reach of the shop's storefront.
#[evento::aggregate(name = "timada-google/Connection")]
pub enum Connection {
    /// A Google account said yes to the platform's application. It replaces
    /// the grant held before, whole: a wider one asked for later covers
    /// what the earlier one did.
    Connected {
        /// The shop: the stream's id does not give it back.
        tenant_id: String,
        /// What renews the access, sealed by the host for this shop. The
        /// access itself lasts an hour and is never recorded.
        refresh_token_sealed: Vec<u8>,
        /// The account's e-mail address, for the merchant to tell which one
        /// it is.
        account: String,
        /// The kinds of access the account granted, each one of
        /// [`ACCESSES`](crate::ACCESSES).
        accesses: Vec<String>,
        connected_by: String,
    },
    /// Which of the account's Search Console sites is this shop's, as
    /// Search Console names it (`https://shop.example/` or
    /// `sc-domain:shop.example`).
    SiteChosen { site: String, chosen_by: String },
    /// Google no longer honours the grant (withdrawn by the account, or
    /// left unused too long): nothing is read through it until the merchant
    /// connects again.
    Lapsed { noticed_by: String },
    /// The merchant took the connection off: the grant is forgotten.
    Disconnected { disconnected_by: String },
}
