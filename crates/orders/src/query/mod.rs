//! Read side. An order's or a cart's own stream is read directly on its page
//! (so a change never waits on a projection). Listing a tenant's orders and
//! carts needs an index a relational store is good at:
//! [`read_model_subscription`] keeps `orders_orders` and `orders_carts`
//! current from every tenant's streams into one database (every row carries
//! the tenant id).

mod carts;
mod migrations;
mod orders;
mod read_model;

pub use carts::*;
pub use migrations::*;
pub use orders::*;
pub use read_model::*;
