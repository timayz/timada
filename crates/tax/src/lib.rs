//! Timada tax assessment.
//!
//! Prices in Timada are **tax-inclusive** (EU B2C style): the storefront shows
//! gross prices, and checkout *extracts* the tax portion for the destination
//! country. A [`TaxCalculator`] performs that assessment; the order snapshots
//! the resulting per-line net/tax amounts into its events so invoices never
//! recompute tax with rates that may have changed since the purchase.
//!
//! [`FixedRateVat`] is the built-in implementation: one default rate plus
//! per-country overrides. External services (TaxJar, Avalara, …) plug in the
//! same way payment providers and suppliers do.

mod calculator;
mod fixed_rate;

pub use calculator::{
    TaxAssessment, TaxAssessmentRequest, TaxCalculator, TaxError, TaxableLine, TaxedLine,
};
pub use fixed_rate::FixedRateVat;
