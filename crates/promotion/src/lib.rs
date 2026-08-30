//! Timada promotions.
//!
//! A [`Discount`] is a code the customer types at the cart: a percentage or a
//! fixed amount off the tax-inclusive total, valid inside a window, optionally
//! capped by a usage limit. The aggregate id derives from the uppercased code
//! ([`discount_id`]), so a code is a natural key and creating it twice is
//! refused rather than duplicated.
//!
//! Usage counting is deliberately **not** an event count. Concurrent checkouts
//! racing the same last redemption slot would both pass an event-count check;
//! the `discount_redemptions` counter row updated under `BEGIN IMMEDIATE`
//! ([`redeem`]) makes the decrement atomic — the same write-side-state
//! precedent as the invoice number sequence. Checkout calls `redeem` *before*
//! committing `OrderPlaced`: a crash in between leaks one redemption slot,
//! which is cheaper than the alternative of over-redeeming a capped campaign.

mod aggregate;
mod commands;
mod migrations;
mod projections;
mod redeem;
mod routes;
mod state;
mod validate;
mod view;

pub use aggregate::{Discount, DiscountCreated, DiscountDisabled, DiscountKind};
pub use commands::{CreateDiscountError, create_discount, disable_discount, discount_id};
pub use migrations::migrations;
pub use projections::{
    AdminDiscountRow, READ_MODELS_SUBSCRIPTION, read_models_subscription, recent_discounts,
    start_subscriptions,
};
pub use redeem::redeem;
pub use routes::admin_router;
pub use state::PromotionState;
pub use validate::{DiscountRefusal, discount_amount, validate};
pub use view::{DiscountView, load_discount};
