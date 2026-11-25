//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// A threshold, or a figure a checkpoint was read with: its name and its
/// value as a person reads it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct Figure {
    pub name: String,
    pub value: String,
}

/// A shop's launch: its target, its checkpoints and what was decided at
/// each. One a shop ([`launch_id`](crate::launch_id)).
#[evento::aggregate(name = "timada-launch/Launch")]
pub enum Launch {
    /// The merchant set what the shop is to reach: a profit after ads a
    /// month, by a day. Set again, the one before stays in the stream: a
    /// change is recorded, not overwritten.
    TargetSet {
        /// A month's profit once the ads are paid, in hundredths.
        monthly_profit_cents: u64,
        currency: String,
        /// `2027-06-30`.
        by_day: String,
        set_by: String,
    },
    /// The thresholds the checkpoints are judged on, each named as
    /// [`THRESHOLDS`](crate::THRESHOLDS) names it.
    ThresholdsSet {
        thresholds: Vec<Figure>,
        set_by: String,
    },
    /// A checkpoint's day came: the numbers it reads, as they were then,
    /// and what they say.
    CheckpointReached {
        /// The day it is counted at: one of
        /// [`CHECKPOINTS`](crate::CHECKPOINTS).
        checkpoint: u32,
        /// Unix seconds the days are counted from: when the first campaign
        /// went live.
        started_at: u64,
        figures: Vec<Figure>,
        /// One of [`ADVICES`](crate::ADVICES).
        advice: String,
        /// Why, in a sentence.
        why: String,
        reached_by: String,
    },
    /// What the merchant decided at a checkpoint, and why.
    DecisionRecorded {
        checkpoint: u32,
        /// One of [`ADVICES`](crate::ADVICES).
        decision: String,
        note: String,
        decided_by: String,
    },
    /// A member says a step of the launch path holds, one timada cannot
    /// check by itself (a legal page's content, a phone that answers): one
    /// of [`CONFIRMED_STEPS`](crate::CONFIRMED_STEPS).
    StepConfirmed { step: String, confirmed_by: String },
    /// A member takes that word back.
    StepUnconfirmed {
        step: String,
        unconfirmed_by: String,
    },
    /// What the launch path holds the catalog to: how many products every
    /// category at the bottom of the tree is to hold, and how many the
    /// shop.
    PathTargetsSet {
        products_per_category: u32,
        products: u32,
        set_by: String,
    },
}

/// The notice, to a shop's owners, that a checkpoint's day came. Its id is
/// made of the checkpoint ([`notice_id`](crate::notice_id)), so each is
/// told once. It is mailed by a subscription on every node of the work
/// group: the node that writes the claim first is the one that mails, then
/// records how it went ([`timada_core::Sending`]).
#[evento::aggregate(name = "timada-launch/Notice")]
pub enum Notice {
    /// What the stream is about, written with the first claim: the notice
    /// of `checkpoint`, worth sending until `expires_at` (unix seconds).
    Owed { checkpoint: u32, expires_at: u64 },
    /// Node `by` took the sending, until `until` should it never say how it
    /// went.
    MailClaimed { by: String, until: u64 },
    /// The notice was handed to the mail server, to every owner.
    MailSent,
    /// The mail server refused every owner's address for good.
    MailUndeliverable { reason: String },
    /// A send failed for a reason worth a retry, after `attempts` tries in
    /// all; the next may be made at `retry_at` (unix seconds).
    MailDeferred {
        retry_at: u64,
        attempts: u32,
        reason: String,
    },
    /// Given up on after `attempts` tries.
    MailFailed { reason: String, attempts: u32 },
}
