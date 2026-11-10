//! What a stream knows of the mail its latest intent owes: a sign-in code, an
//! invitation link, a newsletter confirmation.
//!
//! The command that records the intent mails nothing: a subscription on
//! every node sees the event, and the node that writes the claim first is the
//! one that sends (#427, stage 3 of the CQRS plan). The claim is a lease: a
//! node that dies between its claim and its send loses the mail to the next
//! node that looks once [`LEASE_SECS`] passed. A send that fails for a
//! reason worth a retry (the server did not answer, a transient refusal) is
//! recorded as deferred, with when the next try may be made, and whichever
//! node looks then takes it; the waits grow ([`RETRY_DELAYS_SECS`]), and
//! once they run out the mail is given up on (`Failed`). So a shop whose
//! server is down costs a few seconds of one node's time every so often,
//! never the pass of a subscription (#464). The outcome is recorded on the
//! same stream, so a replayed event finds it and does nothing, and the next
//! intent on the stream starts afresh.

/// How long a claim on a mail holds while nothing is recorded past it. A
/// send, its timeout included, ends well within it; past it the node is
/// taken for dead.
pub const LEASE_SECS: u64 = 300;

/// How long after each failed try the next waits, in seconds: half a minute,
/// then a minute, two, five, a quarter of an hour, an hour, four and twelve.
/// A send that fails after the last is given up on: a day without a mail
/// server is a setup to fix, not a blip to wait out. A sign-in code expires
/// before the later tries, and its claim says so; an invitation and a
/// newsletter confirmation stay open for days.
pub const RETRY_DELAYS_SECS: [u64; 8] = [30, 60, 120, 300, 900, 3_600, 4 * 3_600, 12 * 3_600];

/// Where the mail of a stream's latest intent stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Sending {
    /// Recorded; no node has taken the send yet.
    #[default]
    Pending,
    /// `by` took the send, until `until` (unix seconds) should it never
    /// record how it went; `attempts` sends were tried before this one.
    Claimed {
        by: String,
        until: u64,
        attempts: u32,
    },
    /// Handed to the mail server.
    Sent,
    /// The server refused the recipient's address for good (or it does not
    /// parse): the person has to correct it; `reason` says what the server
    /// said.
    Undeliverable { reason: String },
    /// The send failed for a reason worth a retry (`reason` says which); the
    /// next try may be made at `retry_at` (unix seconds) by whichever node
    /// looks then; `attempts` sends were tried so far.
    Deferred {
        retry_at: u64,
        attempts: u32,
        reason: String,
    },
    /// Given up on after `attempts` sends, the last failing with `reason`:
    /// the mail will not go out unless a new intent is recorded.
    Failed { reason: String, attempts: u32 },
}

/// Who asks to send a mail, and when (unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Claimant<'a> {
    pub node: &'a str,
    pub now: u64,
}

/// What a node asking to send should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// Write the claim, then send.
    Take,
    /// This node holds the claim already (a send it retries): send.
    Held,
    /// Another node is sending right now, the next try is not due yet, or
    /// the outcome is recorded.
    Leave,
}

impl Sending {
    /// Whether the mail went out, or will not.
    pub fn is_settled(&self) -> bool {
        matches!(
            self,
            Self::Sent | Self::Undeliverable { .. } | Self::Failed { .. }
        )
    }

    /// Whether the latest mail could not be delivered to the address: a
    /// fresh intent may be recorded at once, the throttle notwithstanding,
    /// so a corrected address is tried without waiting.
    pub fn is_undeliverable(&self) -> bool {
        matches!(self, Self::Undeliverable { .. })
    }

    /// How many sends were tried so far.
    pub fn attempts(&self) -> u32 {
        match self {
            Self::Pending | Self::Sent | Self::Undeliverable { .. } => 0,
            Self::Claimed { attempts, .. }
            | Self::Deferred { attempts, .. }
            | Self::Failed { attempts, .. } => *attempts,
        }
    }

    /// What `node` should do with the send at `now` (unix seconds).
    pub fn claim(&self, node: &str, now: u64) -> Claim {
        match self {
            Self::Pending => Claim::Take,
            Self::Claimed { by, .. } if by == node => Claim::Held,
            Self::Claimed { until, .. } if now >= *until => Claim::Take,
            Self::Deferred { retry_at, .. } if now >= *retry_at => Claim::Take,
            Self::Claimed { .. }
            | Self::Deferred { .. }
            | Self::Sent
            | Self::Undeliverable { .. }
            | Self::Failed { .. } => Claim::Leave,
        }
    }

    /// The state once `by` took the send until `until`: what a `MailClaimed`
    /// event folds to.
    pub fn claimed(&self, by: &str, until: u64) -> Self {
        Self::Claimed {
            by: by.to_owned(),
            until,
            attempts: self.attempts(),
        }
    }

    /// The state once the send under way failed for `reason`, worth a retry
    /// at `retry_at`: what a `MailDeferred` event folds to.
    pub fn deferred(&self, retry_at: u64, reason: &str) -> Self {
        Self::Deferred {
            retry_at,
            attempts: self.attempts() + 1,
            reason: reason.to_owned(),
        }
    }

    /// The state once the send under way failed for `reason` and no retry
    /// is left: what a `MailFailed` event folds to.
    pub fn failed(&self, reason: &str) -> Self {
        Self::Failed {
            reason: reason.to_owned(),
            attempts: self.attempts() + 1,
        }
    }

    /// When the send under way, failing at `now` (unix seconds), may be
    /// tried again: `None` once the retries ([`RETRY_DELAYS_SECS`]) are
    /// used up, or the next would come after the intent itself lapses at
    /// `expires_at`.
    pub fn retry_at(&self, now: u64, expires_at: u64) -> Option<u64> {
        RETRY_DELAYS_SECS
            .get(self.attempts() as usize)
            .map(|delay| now + delay)
            .filter(|retry_at| *retry_at < expires_at)
    }

    /// Whether `node` holds the claim on the send under way.
    pub fn is_held_by(&self, node: &str) -> bool {
        matches!(self, Self::Claimed { by, .. } if by == node)
    }

    /// What a command records on the stream for `outcome`, the intent
    /// lapsing at `expires_at` (unix seconds): the event to write, as the
    /// state it folds to, or [`Record::Nothing`] when the mail is settled
    /// already, or the node reporting a failure no longer holds the claim (its lease lapsed and another node took the send: its
    /// word on the server is stale, and would count a try that node never
    /// made). A send that did go out is recorded whoever reports it: the
    /// mail is in the shopper's hands.
    pub fn record(&self, outcome: Outcome<'_>, expires_at: u64) -> Record {
        if self.is_settled() {
            return Record::Nothing;
        }
        match outcome {
            Outcome::Sent => Record::Sent,
            Outcome::Undeliverable(reason) => Record::Undeliverable(reason.to_owned()),
            Outcome::NotSent { node, reason, now } => {
                if !self.is_held_by(node) {
                    return Record::Nothing;
                }
                match self.retry_at(now, expires_at) {
                    Some(retry_at) => Record::Deferred {
                        retry_at,
                        reason: reason.to_owned(),
                    },
                    None => Record::Failed {
                        reason: reason.to_owned(),
                    },
                }
            }
        }
    }
}

/// How a send went, as the node that made it reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome<'a> {
    /// Handed to the server.
    Sent,
    /// The server refused the address for good (or it does not parse);
    /// what it said.
    Undeliverable(&'a str),
    /// Anything else, by `node` at `now` (unix seconds): `reason` says
    /// what.
    NotSent {
        node: &'a str,
        reason: &'a str,
        now: u64,
    },
}

/// What to write for an [`Outcome`]: each context has an event per
/// variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Sent,
    Undeliverable(String),
    Deferred {
        retry_at: u64,
        reason: String,
    },
    Failed {
        reason: String,
    },
    /// Nothing: settled already, or a stale report.
    Nothing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_is_taken_once_and_taken_over_once_its_lease_lapsed() {
        let now = 1_000;
        assert_eq!(Sending::Pending.claim("node-1", now), Claim::Take);
        let claimed = Sending::Pending.claimed("node-1", now + LEASE_SECS);
        assert_eq!(claimed.claim("node-1", now), Claim::Held);
        assert_eq!(claimed.claim("node-2", now), Claim::Leave);
        assert_eq!(claimed.claim("node-2", now + LEASE_SECS - 1), Claim::Leave);
        assert_eq!(claimed.claim("node-2", now + LEASE_SECS), Claim::Take);
        assert_eq!(Sending::Sent.claim("node-1", now), Claim::Leave);
        let refused = Sending::Undeliverable {
            reason: "550 no such user".into(),
        };
        assert_eq!(refused.claim("node-1", now), Claim::Leave);
        assert!(Sending::Sent.is_settled());
        assert!(refused.is_settled());
        assert!(refused.is_undeliverable());
        assert!(!Sending::Sent.is_undeliverable());
        assert!(!claimed.is_settled());
        assert!(!Sending::Pending.is_settled());
    }

    /// A send that fails waits longer each time, any node may take the next
    /// try once it is due, and the last failure gives the mail up.
    #[test]
    fn a_failing_send_is_deferred_with_growing_waits_then_given_up() {
        let now = 1_000;
        let mut mail = Sending::Pending.claimed("node-1", now + LEASE_SECS);
        for (tried, delay) in RETRY_DELAYS_SECS.iter().enumerate() {
            let retry_at = mail.retry_at(now, u64::MAX).expect("a retry is left");
            assert_eq!(retry_at, now + delay, "try {}", tried + 1);
            mail = mail.deferred(retry_at, "connection refused");
            assert_eq!(mail.attempts(), tried as u32 + 1);
            assert!(!mail.is_settled());
            assert_eq!(mail.claim("node-2", retry_at - 1), Claim::Leave);
            assert_eq!(mail.claim("node-1", retry_at - 1), Claim::Leave);
            assert_eq!(mail.claim("node-2", retry_at), Claim::Take);
            // The claim carries the count on.
            mail = mail.claimed("node-2", retry_at + LEASE_SECS);
            assert_eq!(mail.attempts(), tried as u32 + 1);
            assert_eq!(mail.claim("node-2", retry_at), Claim::Held);
        }
        assert_eq!(mail.retry_at(now, u64::MAX), None);
        let failed = mail.failed("connection refused");
        assert_eq!(
            failed,
            Sending::Failed {
                reason: "connection refused".into(),
                attempts: RETRY_DELAYS_SECS.len() as u32 + 1,
            }
        );
        assert!(failed.is_settled());
        assert!(!failed.is_undeliverable());
        assert_eq!(failed.claim("node-1", now + 1_000_000), Claim::Leave);
    }

    /// A retry that would come after the intent lapsed is no retry: the
    /// mail is given up on at once.
    #[test]
    fn no_retry_is_scheduled_past_the_intents_expiry() {
        let now = 1_000;
        let claimed = Sending::Pending.claimed("node-1", now + LEASE_SECS);
        assert_eq!(
            claimed.retry_at(now, now + RETRY_DELAYS_SECS[0] + 1),
            Some(now + RETRY_DELAYS_SECS[0])
        );
        assert_eq!(claimed.retry_at(now, now + RETRY_DELAYS_SECS[0]), None);
        let not_sent = |node| Outcome::NotSent {
            node,
            reason: "no answer",
            now,
        };
        assert_eq!(
            claimed.record(not_sent("node-1"), now + 600),
            Record::Deferred {
                retry_at: now + RETRY_DELAYS_SECS[0],
                reason: "no answer".into(),
            }
        );
        let late = Sending::Pending.claimed("node-1", now + LEASE_SECS);
        let late = late
            .deferred(now, "x")
            .deferred(now, "x")
            .deferred(now, "x")
            .deferred(now, "x");
        let late = late.claimed("node-1", now + LEASE_SECS);
        assert_eq!(
            late.record(not_sent("node-1"), now + 600),
            Record::Failed {
                reason: "no answer".into(),
            },
            "the fifth wait (15 min) is past a 10-minute code"
        );
    }

    /// A node whose lease lapsed does not record a failure over another
    /// node's claim, but a mail that went out is recorded by whoever sent
    /// it; nothing is recorded over a settled mail.
    #[test]
    fn a_stale_holder_records_no_failure() {
        let now = 1_000;
        let theirs = Sending::Pending.claimed("node-2", now + LEASE_SECS);
        let mine = Outcome::NotSent {
            node: "node-1",
            reason: "no answer",
            now,
        };
        assert_eq!(theirs.record(mine, u64::MAX), Record::Nothing);
        assert_eq!(theirs.record(Outcome::Sent, u64::MAX), Record::Sent);
        assert_eq!(
            theirs.record(Outcome::Undeliverable("550 no such user"), u64::MAX),
            Record::Undeliverable("550 no such user".into())
        );
        assert_eq!(
            Sending::Sent.record(Outcome::Sent, u64::MAX),
            Record::Nothing
        );
        assert!(theirs.is_held_by("node-2"));
        assert!(!theirs.is_held_by("node-1"));
        assert!(!Sending::Pending.is_held_by("node-1"));
    }
}
