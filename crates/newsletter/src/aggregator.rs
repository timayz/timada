//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.
//!
//! The address is personal data, and events cannot be erased: it is recorded
//! sealed under a key of the subscriber's own ([`timada_core::Key`]), which
//! [`Subscriber::Forgotten`] destroys.

/// One address on a shop's list. Its id is
/// [`subscriber_id`](crate::subscriber_id) of the address, so the stream is
/// also what keeps an address on the list once.
#[evento::aggregate(name = "timada-newsletter/Subscriber")]
pub enum Subscriber {
    /// Asked for the shop's news, to be confirmed through the mailed link.
    /// Asked again (the link was lost, or after leaving the list), it is
    /// written again, under the same key while the subscriber is not erased.
    Requested {
        /// Names the key `email` is sealed under, kept outside the store.
        key_id: String,
        /// The normalized address, sealed under the key, bound to the
        /// subscriber id, the key id and the field's name.
        email: Vec<u8>,
        /// The primary subtag of the shopper's language (`fr`), to mail them
        /// in it. Not personal data: kept in clear.
        language: Option<String>,
        /// Where they asked (`form`, `checkout`, `account`).
        source: String,
        requested_by: String,
    },
    /// The owner of the address followed the link, or proved the address
    /// otherwise (a sign-in with a mailed code).
    Confirmed { confirmed_by: String },
    /// Left the list: through a mail's link, their account, or the merchant.
    Unsubscribed { unsubscribed_by: String },
    /// Erased: the key is destroyed, which leaves the address unreadable for
    /// good.
    Forgotten {
        key_id: String,
        forgotten_by: String,
    },
}
