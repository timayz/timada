//! Shoppers bounded context: the customers of one shop who chose to sign in.
//!
//! A shopper is the shop's, not the platform's: the same address at two shops
//! is two accounts, each in its tenant's own event store. There is no sign-up:
//! a shopper proves an address with a mailed six-digit code, and the first
//! proof creates the account. The account remembers the addresses it ships to;
//! its orders are the shop's orders placed with that address, guest ones
//! included, because owning the mailbox is what entitles a shopper to them.
//!
//! A shopper may sign out every other browser at once, and may delete their
//! account: every session ends, while the orders stay with the shop.
//!
//! Events cannot be erased, so they never hold a shopper's personal data in
//! clear. Each registration draws a random [`ShopperKey`] (the shared
//! [`timada_core::keys`]), and the address
//! signed in with and the addresses shipped to are recorded sealed under it.
//! The crate never stores a key: the caller keeps them through
//! [`ShopperKeys`] and destroys the shopper's key when the account is
//! deleted, which leaves what the account recorded unreadable for good
//! (crypto-shredding). The account is then gone for every command, and the
//! address signs in afresh under a new key. What remains are ids:
//! [`shopper_id`] and [`challenge_id`] are plain hashes of the address.
//!
//! Like the other contexts, the crate is pure domain code: no HTTP, no mail
//! (the caller mails the code [`Command::issue_code`] hands back), no cookies
//! (the caller keeps the session token [`Command::sign_in`] hands back).

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use timada_core::keys::new_key_id;
pub use timada_core::{Key as ShopperKey, Keys as ShopperKeys};
pub use value_object::*;
