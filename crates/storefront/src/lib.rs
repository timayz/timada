//! The headless storefront library: what a merchant's own topcoat app depends
//! on to sell from its timada store.
//!
//! [`Store::connect`] opens the connection to the store API with the tenant's
//! key, keeps a local copy of the published catalog current in the app's own
//! database ([`StoreDb::open`]: one Turso file, SQLite's format in a
//! pure-Rust engine, opened for several writers at once), and
//! [`RouterBuilderStoreExt::store`] puts it in the app context. Pages then read the catalog with [`products`] and
//! [`require_product`] inside their own markup; the one component shipped,
//! [`price`], renders a formatted price and nothing else: every class and
//! attribute comes from the merchant. Every amount a person reads (a cart's
//! totals, a shipping price, an offer's price) is written the way the host
//! writes its own prices when it hands a [`MoneyFormat`] to
//! [`StoreConfig::money`], and as `12.50 €` otherwise.
//!
//! Events stay on the store; only the subscription's cursor, the projection
//! snapshots and the product rows live in the local database (one per replica
//! of the app), so replicas never fence each other out and nothing accumulates
//! on the store.
//!
//! The library declares no page of its own: a library's routes must be added
//! by the host (`#[page]` discovery is the host's), so the only HTTP endpoint,
//! `GET {prefix}/status`, is registered explicitly under the prefix the host
//! chooses.

mod announcements;
mod articles;
mod automations;
mod cart;
mod components;
mod config;
mod consent;
mod context;
mod db;
mod error;
mod google_tag;
mod head;
mod invoice;
mod languages;
mod meta_pixel;
mod migrations;
mod money;
mod newsletter;
mod newsletter_popup;
mod pages;
mod places;
mod query;
mod read_in;
mod read_model;
mod redirects;
mod reviews;
mod router;
mod shoppers;
mod sitemap;
mod source;
mod store;
mod support;
mod terms;
mod translations;
mod wordings;
mod words;

pub use announcements::*;
pub use articles::*;
pub use automations::*;
pub use cart::*;
pub use components::*;
pub use config::*;
pub use consent::*;
pub use context::*;
pub use db::*;
pub use error::*;
pub use google_tag::*;
pub use head::*;
pub use invoice::*;
pub use languages::*;
pub use meta_pixel::*;
pub use migrations::*;
pub use money::*;
pub use newsletter::*;
pub use newsletter_popup::*;
pub use pages::*;
pub use places::*;
pub use query::*;
pub use read_in::{AddressLanguage, ReadIn, Slugs, read_in, shopper_language};
pub use read_model::*;
pub use redirects::*;
pub use reviews::*;
pub use router::*;
pub use shoppers::*;
pub use sitemap::*;
pub use source::*;
pub use store::*;
pub use support::*;
pub use terms::*;
/// How long delivery takes, as [`Product`] and [`CartView`] carry it.
pub use timada_catalog::DeliveryDays;
pub use timada_core::Rendition;
/// A day from Unix seconds, for a host to write an article's date its own
/// way: `(year, month, day)` in UTC, and `YYYY-MM-DD`.
pub use timada_core::civil::{civil_date, format_date};
pub use translations::*;
pub use words::*;
