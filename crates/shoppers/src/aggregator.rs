//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. No field is an
//! enum, so the lock never freezes one.
//!
//! What would name a person (the address signed in with, the addresses
//! shipped to) is sealed under the shopper's own key ([`crate::ShopperKey`]),
//! which the store never holds. Deleting the account destroys the key, so
//! the events below keep nothing readable about the shopper.

/// A shop's customer. Its id is [`shopper_id`](crate::shopper_id) of the
/// address, so the stream doubles as the address's uniqueness index.
#[evento::aggregate(name = "timada-shoppers/Shopper")]
pub enum Shopper {
    /// The first sign-in that proved an address (normalized). `email` is
    /// that address sealed under the key `key_id` names, drawn for this
    /// registration.
    Registered {
        key_id: String,
        email: Vec<u8>,
    },
    /// An address to ship to again. `address_id` derives from its fields
    /// keyed by the shopper's key, so saving the same address twice names
    /// the same entry; `address` is the `ShippingAddress` sealed under it.
    AddressSaved {
        address_id: String,
        address: Vec<u8>,
    },
    AddressRemoved {
        address_id: String,
    },
    /// Every session started up to this event's time is void, except the one
    /// named (the browser that asked to sign out everywhere else).
    SessionsRevoked {
        except_session: Option<String>,
    },
    /// The shopper deleted their account: the key `key_id` names is
    /// destroyed, so what it sealed is unreadable, and every session is
    /// void. Their orders stay with the shop. A later sign-in with the same
    /// address registers it afresh on this stream, under a new key.
    Forgotten {
        key_id: String,
    },
}

/// The one-time codes mailed to one address. Its id is
/// [`challenge_id`](crate::challenge_id) of the address. Only the hash of a
/// code is kept; guesses are counted here so no client can reset them.
#[evento::aggregate(name = "timada-shoppers/ShopperChallenge")]
pub enum ShopperChallenge {
    /// A fresh code, replacing any earlier one. `code_tag` is the code's MAC
    /// under a key the host holds ([`timada_core::CodeVerifier`]): the code
    /// itself never enters the store, and whoever reads the stream cannot
    /// try the million codes against it. Times are unix seconds.
    CodeIssued {
        code_tag: Vec<u8>,
        issued_at: u64,
        expires_at: u64,
    },
    /// A wrong guess against the current code.
    AttemptFailed,
    /// The current code was used.
    CodeConsumed,
}

/// A signed-in browser. Its id is [`session_id`](crate::session_id) of the
/// token the browser holds; the token itself is never stored.
#[evento::aggregate(name = "timada-shoppers/ShopperSession")]
pub enum ShopperSession {
    /// `expires_at` is unix seconds.
    SessionStarted {
        shopper_id: String,
        expires_at: u64,
    },
    SessionEnded,
}
