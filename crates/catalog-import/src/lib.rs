//! Catalogue import: fills a tenant's catalog from a [`Snapshot`], a plain
//! description of categories and products (with their pictures, options,
//! variants and facts) that lives in a file, and takes such a snapshot from
//! a WooCommerce shop through its public Store API.
//!
//! The import writes through [`timada_catalog::Command`] like the dashboard's
//! pages do, so every rule of the catalog holds; it is idempotent on slugs
//! (a category or product already there is left alone), which makes a run
//! that stopped halfway resumable. A snapshot carries its pictures as files
//! beside the catalogue (fetched once from the shop, see
//! [`Snapshot::fetch_pictures`]), and the host keeps them where its catalog
//! serves pictures from through the [`Images`] trait, which also fetches
//! the ones a snapshot lacks. Pure domain code: no permission checks, no
//! HTTP server.

mod import;
mod pictures;
mod snapshot;
mod text;
pub mod woocommerce;

pub use import::*;
pub use pictures::*;
pub use snapshot::*;
pub use text::{decode_entities, html_to_text, truncate_chars};
