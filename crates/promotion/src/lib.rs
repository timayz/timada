//! Promotion bounded context: promo codes (`Discount`, a reusable rule with a
//! redemption cap counted in SQL) and "bons d'achat et avoirs" (`Voucher`, a
//! bearer balance redeemed partially against orders).

pub mod aggregator;
mod command;
mod error;
mod migration;
mod process;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use migration::migrations;
pub use process::*;
pub use query::*;
pub use value_object::*;
