//! What a stream knows of the mail its latest intent owes: a sign-in code, an
//! invitation link, a newsletter confirmation.
//!
//! The command that records the intent mails nothing: a subscription on
//! every node sees the event, and the node that writes the claim first is the
//! one that sends (#427, stage 3 of the CQRS plan). The claim is a lease: a
//! node that dies between its claim and its send loses the mail to the next
//! node that looks once [`LEASE_SECS`] passed, and a node whose send failed
//! for a reason worth a retry keeps its own claim and sends again. The
//! outcome is recorded on the same stream, so a replayed event finds it and
//! does nothing, and the next intent on the stream starts afresh.

/// How long a claim on a mail holds while nothing is recorded past it. The
/// retries of a failing send, on the node that holds the claim, happen well
/// within it; past it the node is taken for dead.
pub const LEASE_SECS: u64 = 300;

/// Where the mail of a stream's latest intent stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Sending {
    /// Recorded; no node has taken the send yet.
    #[default]
    Pending,
    /// `by` took the send, until `until` (unix seconds) should it never
    /// record how it went.
    Claimed { by: String, until: u64 },
    /// Handed to the mail server.
    Sent,
    /// The server refused the recipient's address for good (or it does not
    /// parse): the person has to correct it; `reason` says what the server
    /// said.
    Undeliverable { reason: String },
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
    /// Another node is sending right now, or the outcome is recorded.
    Leave,
}

impl Sending {
    /// Whether the mail went out, or will not.
    pub fn is_settled(&self) -> bool {
        matches!(self, Self::Sent | Self::Undeliverable { .. })
    }

    /// Whether the latest mail could not be delivered to the address: a
    /// fresh intent may be recorded at once, the throttle notwithstanding,
    /// so a corrected address is tried without waiting.
    pub fn is_undeliverable(&self) -> bool {
        matches!(self, Self::Undeliverable { .. })
    }

    /// What `node` should do with the send at `now` (unix seconds).
    pub fn claim(&self, node: &str, now: u64) -> Claim {
        match self {
            Self::Pending => Claim::Take,
            Self::Claimed { by, .. } if by == node => Claim::Held,
            Self::Claimed { until, .. } if now >= *until => Claim::Take,
            Self::Claimed { .. } | Self::Sent | Self::Undeliverable { .. } => Claim::Leave,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_send_is_taken_once_and_taken_over_once_its_lease_lapsed() {
        let now = 1_000;
        assert_eq!(Sending::Pending.claim("node-1", now), Claim::Take);
        let claimed = Sending::Claimed {
            by: "node-1".into(),
            until: now + LEASE_SECS,
        };
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
}
