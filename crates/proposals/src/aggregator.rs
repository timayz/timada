//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// One field a proposal would change.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Change {
    /// Which field of the subject, by the name its kind gives it.
    pub field: String,
    /// Its value when the proposal was prepared: what it must still hold
    /// for the change to apply.
    pub before: String,
    /// The value it would take.
    pub after: String,
}

/// A change to the shop that waits for a person. Its id is fresh.
#[evento::aggregate(name = "timada-proposals/Proposal")]
pub enum Proposal {
    Prepared {
        /// One of [`KINDS`](crate::KINDS).
        kind: String,
        /// What it is about: a product's or a category's id, and for a
        /// category to create the one above it (empty for none).
        subject_id: String,
        /// The subject's name when the proposal was prepared, for a list
        /// to show without looking it up.
        subject_name: String,
        changes: Vec<Change>,
        /// Why: the rule and the numbers it read, or the note of whoever
        /// prepared it.
        reason: String,
        /// What prepared it (`person`, `writer`, a rule's or an agent's
        /// name): what the waiting list is grouped by.
        source: String,
        /// Unix seconds after which it is no longer worth approving.
        valid_until: u64,
        prepared_by: String,
    },
    /// A member said yes to `changes`: the ones prepared, each with the
    /// value the member left it at. The host applies them.
    Approved {
        changes: Vec<Change>,
        approved_by: String,
    },
    /// A member said no.
    Refused { reason: String, refused_by: String },
    /// The approved changes were made.
    Applied { applied_by: String },
    /// `field` no longer held what the proposal was prepared against, but
    /// `found`: nothing was written over it.
    FoundOutOfDate {
        field: String,
        found: String,
        noticed_by: String,
    },
    /// The shop refused the approved change; `reason` says why, for the
    /// member.
    ApplyFailed { reason: String, failed_by: String },
}

/// The notice, to the members who may approve, that proposals wait on the
/// shop on a day. Its id is made of the day
/// ([`notice_id`](crate::notice_id)), so the members are told once a day
/// at most. It is mailed by a subscription on every node of the work
/// group: the node that writes the claim first is the one that mails, then
/// records how it went ([`timada_core::Sending`]).
#[evento::aggregate(name = "timada-proposals/Notice")]
pub enum Notice {
    /// What the stream is about, written with the first claim: the notice
    /// of `day` (`2026-11-24`), worth sending until `expires_at` (unix
    /// seconds), after which no retry is made.
    Owed { day: String, expires_at: u64 },
    /// Node `by` took the sending, until `until` should it never say how it
    /// went.
    MailClaimed { by: String, until: u64 },
    /// The notice was handed to the mail server, to every member it is for.
    MailSent,
    /// The mail server refused every member's address for good; `reason` is
    /// what it said of the first.
    MailUndeliverable { reason: String },
    /// A send failed for a reason worth a retry (`reason` says which),
    /// after `attempts` tries in all; the next may be made at `retry_at`
    /// (unix seconds).
    MailDeferred {
        retry_at: u64,
        attempts: u32,
        reason: String,
    },
    /// Given up on after `attempts` tries: the last failed with `reason` and
    /// no retry is left.
    MailFailed { reason: String, attempts: u32 },
}
