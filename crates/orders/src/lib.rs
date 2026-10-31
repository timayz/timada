//! Orders bounded context: the carts shoppers fill and the orders they place.
//!
//! Lives in the tenant's streams of the store, next to the catalog. The
//! dashboard writes the shopper's side (a cart, then the order it becomes)
//! for the storefront, which asks over HTTP about one cart or order at a
//! time ([`wire`] is what travels); the merchant moves the order along from
//! the dashboard (paid, fulfilled, cancelled). The crate is pure domain code:
//! no permission checks and no HTTP.
//!
//! An order keeps what was sold as it was sold: each line copies the
//! product's name, the variant, its code and its price at the moment the
//! order is placed, so a later change to the catalog never rewrites an order.
//! The order of a cart has a deterministic id, so placing it twice lands on
//! the same stream. Its short reference (`K7Q4-MZP2`) is unique within the
//! tenant through an index aggregate, like a product's slug.
//!
//! A cart may carry one discount code (`timada-discounts`). It is priced with
//! the cart each time, so the order holds the lines at full price and the
//! amount taken off them beside, in a `Discounted` written with `Placed`.
//!
//! The merchant's checkout offers (`timada-discounts` too) are priced the
//! same way, but into the lines: while the rest of the cart meets an offer's
//! conditions, one unit of its item is a line of its own at the offer's
//! price, so the order and its invoice show what that unit cost.

pub mod aggregator;
mod command;
mod discount_usage;
mod error;
mod query;
mod stock_usage;
mod value_object;
pub mod wire;

pub use command::*;
pub use discount_usage::*;
pub use error::*;
pub use query::*;
pub use stock_usage::*;
pub use value_object::*;
