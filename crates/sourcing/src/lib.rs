//! Sourcing bounded context: where the shop's products are bought, what they
//! cost there, and — from the purchase orders of a later step — the orders
//! placed with a supplier for a customer's.
//!
//! A [`Supplier`](aggregator::Supplier) is taken on under a permanent slug
//! and a **connector key**: the adapter that talks to it, or `"manual"` when
//! an operator does. A [`SourcedProduct`](aggregator::SourcedProduct) says
//! which supplier item one of the shop's products is bought as — one stream
//! per product, so a product has one supplier at a time by construction.
//!
//! What a supplier *says* is not an event. A feed polled every few hours
//! across a real catalogue would append hundreds of thousands a day and
//! leave posterity none the wiser, so the last word of each supplier lives
//! in `sourcing_offer`, and the consequences that matter are facts in the
//! contexts that own them: the selling price in `timada-pricing`, the level
//! on the shelf in `timada-inventory`.
//!
//! Nor is the [`PricingRule`] — a markup, a rounding, a guardrail — an
//! event. It is configuration, tuned whenever a margin disappoints, and a
//! shape written into `events.lock` could never be tuned again.
//!
//! [`Command::apply_offer`] is where the two meet: it converts a cost with
//! the [`timada_tax::ExchangeRates`] port, prices it by the rule, and moves
//! the selling price when the guardrails allow — otherwise it hands back a
//! [`Verdict::Review`](price::Verdict::Review) for an operator to settle.

pub mod aggregator;
mod command;
pub mod connector;
mod error;
mod migration;
pub mod price;
mod query;
mod rule;
mod sourcing_list;

pub use command::*;
pub use connector::{
    ConnectorError, ConnectorLimits, ConnectorTask, FakeConnector, ManualConnector, PlaceOrder,
    PlacedOrder, PurchaseLine, SupplierConnector, SupplierConnectors, SupplierItemRef,
    SupplierOffer, SupplierOrderStanding,
};
pub use error::*;
pub use migration::migrations;
pub use price::{Quote, ReviewReason, Verdict};
pub use query::*;
pub use rule::*;
pub use sourcing_list::*;
