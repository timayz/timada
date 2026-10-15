//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. No field is an
//! enum, so the lock never freezes one.

use timada_orders::ShippingAddress;

/// A shop's customer. Its id is [`shopper_id`](crate::shopper_id) of the
/// address, so the stream doubles as the address's uniqueness index.
#[evento::aggregate(name = "timada-shoppers/Shopper")]
pub enum Shopper {
    /// The first sign-in that proved `email` (normalized).
    Registered {
        email: String,
    },
    /// An address to ship to again. `address_id` derives from its fields, so
    /// saving the same address twice names the same entry.
    AddressSaved {
        address_id: String,
        address: ShippingAddress,
    },
    AddressRemoved {
        address_id: String,
    },
}

/// The one-time codes mailed to one address. Its id is
/// [`challenge_id`](crate::challenge_id) of the address. Only the hash of a
/// code is kept; guesses are counted here so no client can reset them.
#[evento::aggregate(name = "timada-shoppers/ShopperChallenge")]
pub enum ShopperChallenge {
    /// A fresh code, replacing any earlier one. Times are unix seconds.
    CodeIssued {
        code_hash: String,
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
