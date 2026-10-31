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
    /// Where the subscriber lives, as their browser said when they asked:
    /// an IANA name (`Europe/Paris`), for a campaign to reach them at the
    /// hour it names in their own day. Coarse, so kept in clear.
    TimeZoneSet { time_zone: String, set_by: String },
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
    /// Every subscriber confirmed when it went out was mailed (or tried);
    /// of a scheduled campaign, its last edition went out.
    Finished { finished_by: String },
    /// When it goes out, rather than at once: a preset of the calendar (see
    /// [`crate::schedule::Schedule`]). Days are `YYYY-MM-DD`, times minutes
    /// into the local day of each subscriber, `cadence` one of
    /// `once`, `daily`, `weekly`, `monthly`. Set on a draft, or on a
    /// scheduled campaign for its editions to come.
    ScheduleSet {
        cadence: String,
        weekdays: Vec<u8>,
        month_day: Option<u8>,
        starts_on: String,
        ends_on: Option<String>,
        window_start: u16,
        window_end: Option<u16>,
        pace_minutes: u16,
        set_by: String,
    },
    /// The draft goes out at once again, when sent.
    ScheduleCleared { cleared_by: String },
    /// The merchant started the schedule: from now on each cycle of it is
    /// an edition, going out to the confirmed subscribers in its slots.
    ScheduleActivated { activated_by: String },
    /// The merchant stopped the schedule: no edition opens any more, and
    /// one going out mails nobody else.
    ScheduleStopped { stopped_by: String },
}

/// One cycle of a scheduled campaign: its copy as it stood when the cycle
/// opened, and its own deliveries. Its id is
/// [`edition_id`](crate::edition_id) of the campaign and its first cycle,
/// so a cycle opens once however many nodes look.
#[evento::aggregate(name = "timada-newsletter/Edition")]
pub enum Edition {
    EditionOpened {
        campaign_id: String,
        /// The first day of the cycle, `YYYY-MM-DD`.
        cycle: String,
        subject: String,
        html: String,
        text: Option<String>,
        language: Option<String>,
        opened_by: String,
    },
    /// A one-off edition still owed mails when its window closed: it goes
    /// on in the next cycle's slots.
    EditionRolled { cycle: String, rolled_by: String },
    /// Its window closed: nobody else gets it.
    EditionClosed { closed_by: String },
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
    /// Taken for one edition of a scheduled campaign; the stream id is
    /// [`delivery_id`](crate::delivery_id) of the edition and subscriber.
    ClaimedForEdition {
        campaign_id: String,
        edition_id: String,
        subscriber_id: String,
        claimed_by: String,
    },
    /// Not mailed, for good: the read model owed the subscriber the mail but
    /// the stream says they left the list (or were erased) before it went
    /// out. Recorded as the first event of the pair's stream, so the
    /// campaign stops owing it; `edition_id` names the edition of a
    /// scheduled campaign, `None` for one sent at once.
    Skipped {
        campaign_id: String,
        edition_id: Option<String>,
        subscriber_id: String,
        reason: String,
        skipped_by: String,
    },
}
