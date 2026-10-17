//! Catalogue import: fills a tenant's catalog from a [`Snapshot`], a plain
//! description of categories and products (with their pictures, options,
//! variants and facts) that lives in a file, and takes such a snapshot from
//! a WooCommerce shop through its public Store API.
//!
//! The import writes through [`timada_catalog::Command`] like the dashboard's
//! pages do, so every rule of the catalog holds; it is idempotent on slugs
//! (a category or product already there is left alone), which makes a run
//! that stopped halfway resumable. Pictures are fetched by the host through
//! the [`Images`] trait, since where they are kept is the host's business.
//! Pure domain code: no permission checks, no HTTP server.

mod import;
mod snapshot;
mod text;
pub mod woocommerce;

pub use import::*;
pub use snapshot::*;
pub use text::{decode_entities, html_to_text, truncate_chars};
