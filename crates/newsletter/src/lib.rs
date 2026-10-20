//! Newsletter bounded context: the shoppers who asked a shop for its news.
//!
//! An address joins a shop's list in two steps (double opt-in): it is
//! *requested* from the storefront (its footer, the checkout, the shopper's
//! account), and counts only once its owner follows the link mailed to it.
//! A shopper signed in with a mailed code has proved their address already,
//! so their subscription is confirmed at once. Anyone holding the link of a
//! mail they got may leave the list again.
//!
//! The links carry no secret of their own: each is the subscriber's id with
//! a MAC under the subscriber's key, so the store keeps nothing a link could
//! be read back from, and erasing the subscriber kills them too.
//!
//! Events cannot be erased, so they never hold the address in clear: each
//! subscriber draws a random key ([`timada_core::Key`]) and the address is
//! recorded sealed under it. The crate never stores a key: the caller keeps
//! them through [`timada_core::Keys`] and destroys the subscriber's key when
//! they are erased ([`Command::forget_subscriber`]). The language is kept
//! in clear. The crate is pure domain code: no permission checks, no HTTP,
//! no mail, no rate limiting.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
