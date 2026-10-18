//! AliExpress bounded context: the items a shop imports from AliExpress's
//! dropshipping catalogue, and keeping their price, stock and availability
//! in step.
//!
//! The merchant's own AliExpress app (its key and sealed secret, and the
//! tokens of the account it acts for) is a setting of the tenant, in the
//! platform store (`timada-tenancy`). This crate holds the calls to
//! AliExpress's open platform ([`Client`], signed as moh3a/ae_sdk does),
//! what a product maps to in the catalog ([`map_product`]), what a read
//! changes there ([`apply_sync`]), and one stream per imported item in the
//! tenant's store, so a shop imports an item once and the dashboard knows
//! what each variant is on AliExpress.

pub mod aggregator;
mod apply;
mod client;
mod command;
mod error;
mod map;
mod model;
mod query;
mod value_object;

pub use apply::*;
pub use client::*;
pub use command::*;
pub use error::*;
pub use map::*;
pub use model::*;
pub use query::*;
pub use value_object::*;
