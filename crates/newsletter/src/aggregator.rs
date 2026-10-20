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

/// One mail of news the merchant writes, then sends to the confirmed
/// subscribers. Its id is a fresh [`new_campaign_id`](crate::new_campaign_id).
/// The copy is the merchant's, not personal data: kept in clear.
#[evento::aggregate(name = "timada-newsletter/Campaign")]
pub enum Campaign {
    /// Written as a draft. `subject`, `html` and `text` are templates over
    /// the shop's variables and the subscriber's unsubscribe link, checked
    /// when saved.
    Drafted {
        subject: String,
        html: String,
        /// The plain-text part; without it, the text is derived from the
        /// HTML.
        text: Option<String>,
        /// Only the subscribers in this language (a primary subtag) get it;
        /// none is everyone.
        language: Option<String>,
        drafted_by: String,
    },
    /// The draft rewritten, whole.
    Edited {
        subject: String,
        html: String,
        text: Option<String>,
        language: Option<String>,
        edited_by: String,
    },
    /// The merchant sent it: from now on it goes out to every confirmed
    /// subscriber (in its language), a batch at a time, and can no longer be
    /// edited.
    SendRequested { requested_by: String },
    /// Every subscriber confirmed when it went out was mailed (or tried).
    Finished { finished_by: String },
}

/// One campaign's mail to one subscriber. One stream per pair, with the id
/// [`delivery_id`](crate::delivery_id): the node that writes `Claimed`
/// first is the one that mails, so a subscriber gets a campaign once
/// however many nodes send it.
#[evento::aggregate(name = "timada-newsletter/Delivery")]
pub enum Delivery {
    Claimed {
        campaign_id: String,
        subscriber_id: String,
        claimed_by: String,
    },
    /// The mail could not be handed to the mail server; it is not tried
    /// again.
    Failed { reason: String },
}
