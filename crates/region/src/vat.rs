//! [`RegionVat`] — the region-backed `TaxCalculator`.
//!
//! Same tax-inclusive extraction as `timada_tax::FixedRateVat`, but the rates
//! come from the `region_countries` read table, so they are admin-edited
//! configuration. Two extra behaviours fall out of the region model:
//!
//! - An empty or unclaimed country falls back to the default rate, preserving
//!   the checkout preview's contract (it assesses with `country: ""`).
//! - A claimed country whose region's currency differs from the cart's is
//!   refused with `UnsupportedCountry`: the customer is checking out in a
//!   currency that region does not trade in, and checkout already turns that
//!   error into a 400 with a readable reason.

use sqlx::SqlitePool;
use timada_core::Money;
use timada_tax::{TaxAssessment, TaxAssessmentRequest, TaxCalculator, TaxError, TaxedLine};

use crate::projections::region_for_country;

#[derive(Debug, Clone)]
pub struct RegionVat {
    read_pool: SqlitePool,
    default_rate_bps: u32,
}

impl RegionVat {
    pub fn new(read_pool: SqlitePool, default_rate_bps: u32) -> Self {
        Self {
            read_pool,
            default_rate_bps,
        }
    }
}

/// Extract the net portion of a tax-inclusive amount, rounding half-up —
/// the same maths as `FixedRateVat`, kept in lockstep on purpose.
fn net_of_gross(gross_cents: i64, rate_bps: u32) -> i64 {
    let divisor = i128::from(10_000_u32 + rate_bps);
    let scaled = i128::from(gross_cents) * 10_000;
    let net = (scaled + divisor / 2) / divisor;
    // A cent amount divided by a value >= 10000/10000 stays within i64.
    net as i64
}

#[async_trait::async_trait]
impl TaxCalculator for RegionVat {
    fn id(&self) -> &'static str {
        "region-vat"
    }

    async fn assess(&self, req: TaxAssessmentRequest) -> Result<TaxAssessment, TaxError> {
        let currency = match req.lines.first() {
            Some(line) => line.gross_unit_price.currency,
            // An empty assessment is a caller edge case (checkout rejects
            // empty carts); zero in the default currency keeps it total-safe.
            None => Money::default().currency,
        };

        let country = req.country.trim().to_uppercase();
        let rate_bps = if country.is_empty() {
            self.default_rate_bps
        } else {
            match region_for_country(&self.read_pool, &country)
                .await
                .map_err(|source| TaxError::Api(source.to_string()))?
            {
                Some((region, rate)) => {
                    if region.currency != currency.code() {
                        return Err(TaxError::UnsupportedCountry(
                            country,
                            format!(
                                "{} orders ship in {}, but this cart is in {} — \
                                 switch region or empty the cart",
                                region.name, region.currency, currency
                            ),
                        ));
                    }
                    rate
                }
                // Nobody claims the country: the store still sells there at
                // the default rate, mirroring FixedRateVat's fallback.
                None => self.default_rate_bps,
            }
        };

        let mut lines = Vec::with_capacity(req.lines.len());
        let mut total_net = Money::zero(currency);
        let mut total_tax = Money::zero(currency);
        let mut total_gross = Money::zero(currency);

        for line in req.lines {
            if line.gross_unit_price.currency != currency {
                return Err(TaxError::MixedCurrencies);
            }
            let gross = line.gross_unit_price.multiply(line.quantity);
            let net = Money::new(net_of_gross(gross.amount_cents, rate_bps), currency);
            let tax = Money::new(gross.amount_cents - net.amount_cents, currency);

            total_net = total_net.add(net).map_err(|_| TaxError::MixedCurrencies)?;
            total_tax = total_tax.add(tax).map_err(|_| TaxError::MixedCurrencies)?;
            total_gross = total_gross
                .add(gross)
                .map_err(|_| TaxError::MixedCurrencies)?;

            lines.push(TaxedLine {
                reference: line.reference,
                tax_rate_bps: rate_bps,
                net,
                tax,
                gross,
            });
        }

        Ok(TaxAssessment {
            lines,
            total_net,
            total_tax,
            total_gross,
        })
    }
}
