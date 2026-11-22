//! Events of an invitation to create an account. Every variant and every
//! field is frozen once it ships (bitcode is positional); evolve by adding
//! variants.

/// Root's invitation of one address to create an account, the only way in
/// once the host closed sign-up. The aggregate id is
/// [`crate::account_invitation_id`] of the link's token. Times are unix
/// seconds.
///
/// The token itself never enters the store: the event keeps a random
/// `nonce`, and the token is derived from it under a key the host holds
/// ([`timada_core::TokenDeriver`]), by the node that mails the link. The mail
/// is sent by a subscription, which records how it went on the stream
/// (`MailClaimed`, `MailSent`, `MailUndeliverable`, `MailDeferred`,
/// `MailFailed`; see [`timada_core::Sending`]).
#[evento::aggregate(name = "timada-identity/AccountInvitation")]
pub enum AccountInvitation {
    /// `email` is the normalized address the link goes to, and the only one
    /// it creates an account for.
    Sent {
        email: String,
        invited_by: String,
        nonce: Vec<u8>,
        sent_at: u64,
        expires_at: u64,
    },
    /// The link's one use: `user_id` is the account it creates.
    Accepted {
        user_id: String,
        accepted_at: u64,
    },
    Revoked {
        revoked_by: String,
    },
    /// Node `by` took the sending of the link, until `until` should it never
    /// say how it went.
    MailClaimed {
        by: String,
        until: u64,
    },
    /// The link was handed to the mail server.
    MailSent,
    /// The mail server refused the address for good; `reason` is what it
    /// said.
    MailUndeliverable {
        reason: String,
    },
    /// The send failed for a reason worth a retry (`reason` says which),
    /// after `attempts` tries in all; the next may be made at `retry_at`
    /// (unix seconds).
    MailDeferred {
        retry_at: u64,
        attempts: u32,
        reason: String,
    },
    /// Given up on after `attempts` tries: the last failed with `reason` and
    /// no retry is left.
    MailFailed {
        reason: String,
        attempts: u32,
    },
}
