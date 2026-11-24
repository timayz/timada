//! Google bounded context: the one connection a shop keeps to a Google
//! account, which everything it reads from Google goes through.
//!
//! A merchant says yes once, on Google's own page, to the kinds of access
//! the shop turns on (Search Console, Google Ads, Merchant Center); what
//! Google hands back is kept sealed by the host and recorded here, with the
//! account that gave it and the kinds it covers. Asking for one more kind
//! later replaces the grant with a wider one, without losing what was
//! granted. A grant Google no longer honours is recorded as lapsed, so
//! every page that reads through it says "connect again" until the merchant
//! does.
//!
//! The connection's stream sits in the platform's own tenant (the default
//! tenant of the store), under the shop's id: a storefront's key, which
//! reads the shop's streams, never sees the sealed grant. The crate is pure
//! domain code: it never calls Google and never sees a grant in the clear.

pub mod aggregator;
mod command;
mod error;
mod value_object;

pub use command::*;
pub use error::*;
pub use value_object::*;
