use std::collections::HashMap;

use timada_core::Money;

use crate::calculator::{TaxAssessment, TaxAssessmentRequest, TaxCalculator, TaxError, TaxedLine};

/// Fixed-rate VAT with per-country overrides, for tax-inclusive prices.
///
/// ```
/// use timada_tax::FixedRateVat;
/// let vat = FixedRateVat::new(2000) // 20 % default
///     .with_country("DE", 1900)
///     .with_country("LU", 1700);
/// ```
#[derive(Debug, Clone)]
pub struct FixedRateVat {
    default_rate_bps: u32,
    country_rates_bps: HashMap<String, u32>,
}

impl FixedRateVat {
    pub fn new(default_rate_bps: u32) -> Self {
        Self {
            default_rate_bps,
            country_rates_bps: HashMap::new(),
        }
    }

    pub fn with_country(mut self, country: &str, rate_bps: u32) -> Self {
        self.country_rates_bps
            .insert(country.to_uppercase(), rate_bps);
        self
    }

    fn rate_for(&self, country: &str) -> u32 {
        self.country_rates_bps
            .get(&country.to_uppercase())
            .copied()
            .unwrap_or(self.default_rate_bps)
    }
}

/// Extract the net portion of a tax-inclusive amount, rounding half-up.
///
/// `net = gross * 10000 / (10000 + rate_bps)`; the tax is the remainder, so
/// `net + tax == gross` holds exactly per line.
fn net_of_gross(gross_cents: i64, rate_bps: u32) -> i64 {
    let divisor = i128::from(10_000_u32 + rate_bps);
    let scaled = i128::from(gross_cents) * 10_000;
    let net = (scaled + divisor / 2) / divisor;
    // A cent amount divided by a value >= 10000/10000 stays within i64.
    net as i64
}

#[async_trait::async_trait]
impl TaxCalculator for FixedRateVat {
    fn id(&self) -> &'static str {
        "fixed-rate-vat"
    }

    async fn assess(&self, req: TaxAssessmentRequest) -> Result<TaxAssessment, TaxError> {
        let rate_bps = self.rate_for(&req.country);

        let currency = match req.lines.first() {
            Some(line) => line.gross_unit_price.currency,
            // An empty assessment is a caller edge case (checkout rejects
            // empty carts); zero in the default currency keeps it total-safe.
            None => Money::default().currency,
        };

        let mut lines = Vec::with_capacity(req.lines.len());
        let mut total_net = Money::zero(currency);
        let mut total_tax = Money::zero(currency);
        let mut total_gross = Money::zero(currency);

        for line in req.lines {
            if line.gross_unit_price.currency != currency {
                return Err(TaxError::MixedCurrencies);
            }
            if line.discount.amount_cents != 0 && line.discount.currency != currency {
                return Err(TaxError::MixedCurrencies);
            }
            let gross = Money::new(
                line.gross_unit_price.multiply(line.quantity).amount_cents
                    - line.discount.amount_cents,
                currency,
            );
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

#[cfg(test)]
mod tests {
    use timada_core::Currency;

    use super::*;
    use crate::calculator::TaxableLine;

    fn line(reference: &str, gross_cents: i64, quantity: u32) -> TaxableLine {
        TaxableLine::undiscounted(
            reference.into(),
            Money::new(gross_cents, Currency::Eur),
            quantity,
        )
    }

    #[tokio::test]
    async fn extracts_inclusive_vat_with_stable_rounding() {
        let vat = FixedRateVat::new(2000);
        let assessment = vat
            .assess(TaxAssessmentRequest {
                country: "FR".into(),
                lines: vec![line("a", 1200, 1)],
            })
            .await
            .unwrap();

        // 12.00 gross at 20 % inclusive → 10.00 net + 2.00 tax.
        assert_eq!(assessment.total_net.amount_cents, 1000);
        assert_eq!(assessment.total_tax.amount_cents, 200);
        assert_eq!(assessment.total_gross.amount_cents, 1200);
    }

    #[tokio::test]
    async fn net_plus_tax_equals_gross_even_on_awkward_amounts() {
        let vat = FixedRateVat::new(1900);
        let assessment = vat
            .assess(TaxAssessmentRequest {
                country: "DE".into(),
                lines: vec![line("a", 3499, 1), line("b", 101, 3)],
            })
            .await
            .unwrap();

        for taxed in &assessment.lines {
            assert_eq!(
                taxed.net.amount_cents + taxed.tax.amount_cents,
                taxed.gross.amount_cents,
                "line {} must reconcile",
                taxed.reference
            );
            assert_eq!(taxed.tax_rate_bps, 1900);
        }
        assert_eq!(
            assessment.total_net.amount_cents + assessment.total_tax.amount_cents,
            assessment.total_gross.amount_cents
        );
    }

    #[tokio::test]
    async fn country_overrides_beat_the_default_rate() {
        let vat = FixedRateVat::new(2000).with_country("de", 1900);
        let germany = vat
            .assess(TaxAssessmentRequest {
                country: "DE".into(),
                lines: vec![line("a", 11900, 1)],
            })
            .await
            .unwrap();
        assert_eq!(germany.total_net.amount_cents, 10000);

        let elsewhere = vat
            .assess(TaxAssessmentRequest {
                country: "GB".into(),
                lines: vec![line("a", 12000, 1)],
            })
            .await
            .unwrap();
        assert_eq!(elsewhere.total_net.amount_cents, 10000);
    }

    #[tokio::test]
    async fn mixed_currencies_are_rejected() {
        let vat = FixedRateVat::new(2000);
        let result = vat
            .assess(TaxAssessmentRequest {
                country: "FR".into(),
                lines: vec![
                    line("a", 1000, 1),
                    TaxableLine::undiscounted("b".into(), Money::new(1000, Currency::Usd), 1),
                ],
            })
            .await;
        assert!(matches!(result, Err(TaxError::MixedCurrencies)));
    }
}

#[cfg(test)]
mod discount_tests {
    use timada_core::{Currency, Money};

    use super::*;
    use crate::calculator::{TaxAssessmentRequest, TaxableLine};

    #[tokio::test]
    async fn vat_is_extracted_from_the_discounted_gross() {
        let vat = FixedRateVat::new(2000);
        let assessment = vat
            .assess(TaxAssessmentRequest {
                country: "FR".into(),
                lines: vec![TaxableLine {
                    reference: "a".into(),
                    gross_unit_price: Money::new(1200, Currency::Eur),
                    quantity: 1,
                    discount: Money::new(120, Currency::Eur),
                }],
            })
            .await
            .unwrap();

        // 12.00 − 1.20 = 10.80 gross at 20 % inclusive → 9.00 net + 1.80 tax.
        assert_eq!(assessment.total_gross.amount_cents, 1080);
        assert_eq!(assessment.total_net.amount_cents, 900);
        assert_eq!(assessment.total_tax.amount_cents, 180);
    }
}
