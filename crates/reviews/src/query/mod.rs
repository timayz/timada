//! Read side. A review's own stream is read directly on its page (so a
//! moderation never waits on a projection). Listing a tenant's reviews uses
//! `reviews_reviews`, and a product's rating `reviews_product_ratings`, which
//! [`read_model_subscription`] keeps current from one tenant's event stream
//! into a shared database (every row carries the tenant id).

mod migrations;
mod read_model;
mod reviews;

pub use migrations::*;
pub use read_model::*;
pub use reviews::*;
