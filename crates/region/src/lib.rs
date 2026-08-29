//! Timada regions.
//!
//! A [`Region`] is a Medusa-style currency zone: one currency, a set of member
//! countries, and each country's VAT rate in basis points. The storefront
//! resolves the browser to a region (cookie, falling back to the first region)
//! and prices everything in that region's currency; [`RegionVat`] implements
//! `timada-tax`'s `TaxCalculator` on top of the `region_countries` read table,
//! so checkout keeps its exact contract while the rates become admin-editable
//! configuration instead of app wiring.
//!
//! The read tables are eventually consistent, which is fine for what they hold:
//! region config changes are rare and administrative, and every money amount an
//! order snapshots still comes from the authoritative checkout-time assessment.
//! A country claimed by two regions belongs to whichever wrote last — the
//! `region_countries` primary key makes that deterministic, and the admin form
//! is the place to fix the overlap.

mod aggregate;
mod commands;
mod cookie;
mod migrations;
mod projections;
mod routes;
mod state;
mod vat;
mod view;

pub use aggregate::{Region, RegionCountry, RegionCreated, RegionUpdated};
pub use commands::{RegionError, create_region, update_region};
pub use cookie::{REGION_COOKIE, current_region, region_cookie};
pub use migrations::migrations;
pub use projections::{
    READ_MODELS_SUBSCRIPTION, RegionRow, list_regions, read_models_subscription,
    region_for_country, start_subscriptions,
};
pub use routes::{admin_router, store_router};
pub use state::RegionState;
pub use vat::RegionVat;
pub use view::{RegionView, load_region};
