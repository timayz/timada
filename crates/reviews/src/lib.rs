//! Reviews bounded context: what shoppers think of the products they bought.
//!
//! Only a buyer reviews: a review is written from a paid order, for a product
//! on it, once per product and order (the review's id derives from both). It
//! carries stars, an optional title, a text and a few photos, and nobody sees
//! it until the merchant publishes it; a refused one never comes back. Like
//! the catalog, reviews live in the tenant's streams of the store and the crate is
//! pure domain code: no permission checks, no HTTP, no files (the photos are
//! stored by whoever takes the upload; a review only records what they are).

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
