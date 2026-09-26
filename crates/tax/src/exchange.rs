//! Exchange rates, for the books. A shop may sell in several currencies, but
//! it keeps its accounts — and files its VAT — in one: the **base** currency.
//! An invoice in another currency must state its VAT in the base one, at a
//! rate that can be pointed at: for a French seller, the latest rate the
//! European Central Bank published (CGI ann. III art. 242 nonies A, VAT
//! directive art. 91).
//!
//! Nothing here touches what a customer pays: prices are set per currency,
//! never converted. The rate is *pinned* when the order is placed
//! ([`PinnedRate`], a persisted type), and everything that follows — the
//! invoice, its credit notes, the VAT journal — uses that one rate.
//! Where rates come from is a port, [`ExchangeRates`]: [`FixedRates`], a
//! table the host keeps, or `EcbRates` (feature `ecb`).

use std::{future::Future, pin::Pin, sync::Arc};

use bitcode::{Decode, Encode};
use timada_core::Money;

const MICROS: u128 = 1_000_000;

/// A rate as it was when an order was placed, quoted the way central banks
/// do: how many units of `currency` one unit of `base` buys, in millionths
/// (`853_800` = 1 EUR = 0,8538 GBP).
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct PinnedRate {
    /// The currency of the books.
    pub base: String,
    /// The currency of the sale.
    pub currency: String,
    pub per_base_micros: u64,
    /// Unix seconds: the day the rate is of — a bank publishes on working
    /// days, so it may be older than the order.
    pub as_of: u64,
    /// Who published it: "ECB", or whatever the host's table says.
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RateError {
    /// The source quotes no such pair.
    #[error("no rate for {0}")]
    Unknown(String),
    /// The source could not be reached; asking again may work.
    #[error("rates unavailable: {0}")]
    Unavailable(String),
    #[error("{amount} is not an amount in {expected}")]
    OtherCurrency { amount: String, expected: String },
}

impl PinnedRate {
    /// `amount`, which must be in the rate's currency, in the base currency —
    /// rounded half up to the hundredth.
    pub fn to_base(&self, amount: &Money) -> Result<Money, RateError> {
        if amount.currency != self.currency || self.per_base_micros == 0 {
            return Err(RateError::OtherCurrency {
                amount: amount.currency.clone(),
                expected: self.currency.clone(),
            });
        }
        let quote = u128::from(self.per_base_micros);
        let magnitude = u128::from(amount.minor.unsigned_abs());
        let rounded = (magnitude * MICROS * 2 + quote) / (quote * 2);
        let minor = i64::try_from(rounded).unwrap_or(i64::MAX);
        Ok(Money::new(
            if amount.minor < 0 { -minor } else { minor },
            &self.base,
        ))
    }

    /// `amount`, which must be in the base currency, in the rate's currency
    /// — rounded half up to the hundredth. The way back from
    /// [`PinnedRate::to_base`], for a cost quoted in the books' currency that
    /// has to be read in the one a product is sold in.
    pub fn from_base(&self, amount: &Money) -> Result<Money, RateError> {
        if amount.currency != self.base || self.per_base_micros == 0 {
            return Err(RateError::OtherCurrency {
                amount: amount.currency.clone(),
                expected: self.base.clone(),
            });
        }
        let quote = u128::from(self.per_base_micros);
        let magnitude = u128::from(amount.minor.unsigned_abs());
        let rounded = (magnitude * quote * 2 + MICROS) / (MICROS * 2);
        let minor = i64::try_from(rounded).unwrap_or(i64::MAX);
        Ok(Money::new(
            if amount.minor < 0 { -minor } else { minor },
            &self.currency,
        ))
    }

    /// `1 EUR = 0,8538 GBP`: the quote, as a document prints it — four
    /// decimals, more when the rate needs them.
    pub fn quote(&self) -> String {
        let units = self.per_base_micros / 1_000_000;
        let fraction = format!("{:06}", self.per_base_micros % 1_000_000);
        let kept = fraction.trim_end_matches('0');
        let decimals = if kept.len() < 4 { &fraction[..4] } else { kept };
        format!("1 {} = {units},{decimals} {}", self.base, self.currency)
    }
}

/// The result of a source call, boxed so sources can be `dyn`.
pub type RateFuture<'a> = Pin<Box<dyn Future<Output = Result<PinnedRate, RateError>> + Send + 'a>>;

pub trait ExchangeRates: Send + Sync {
    /// The latest rate published at `at` (Unix seconds) for one unit of
    /// `base` in `currency`.
    fn rate<'a>(&'a self, base: &'a str, currency: &'a str, at: u64) -> RateFuture<'a>;
}

/// The source, in a shape subscriptions can carry.
#[derive(Clone)]
pub struct ExchangeRateSource(pub Arc<dyn ExchangeRates>);

impl ExchangeRateSource {
    pub fn new(source: impl ExchangeRates + 'static) -> Self {
        Self(Arc::new(source))
    }
}

/// A table the host keeps: for tests, demos, and shops whose accountant
/// gives them a monthly rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedRates {
    base: String,
    source: String,
    /// `(currency, millionths of it per unit of base)`.
    rates: Vec<(String, u64)>,
}

impl FixedRates {
    pub fn new(base: &str, source: &str) -> Self {
        Self {
            base: base.to_owned(),
            source: source.to_owned(),
            rates: Vec::new(),
        }
    }

    /// One unit of the base currency buys `per_base_micros` millionths of
    /// `currency`.
    pub fn with(mut self, currency: &str, per_base_micros: u64) -> Self {
        self.rates.retain(|(known, _)| known != currency);
        self.rates.push((currency.to_owned(), per_base_micros));
        self
    }
}

impl ExchangeRates for FixedRates {
    fn rate<'a>(&'a self, base: &'a str, currency: &'a str, at: u64) -> RateFuture<'a> {
        Box::pin(async move {
            let known = self
                .rates
                .iter()
                .find(|(known, per_base)| known == currency && *per_base > 0)
                .filter(|_| base == self.base);
            match known {
                Some((_, per_base_micros)) => Ok(PinnedRate {
                    base: base.to_owned(),
                    currency: currency.to_owned(),
                    per_base_micros: *per_base_micros,
                    as_of: at,
                    source: self.source.clone(),
                }),
                None => Err(RateError::Unknown(format!("{base}/{currency}"))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pounds() -> PinnedRate {
        PinnedRate {
            base: "EUR".into(),
            currency: "GBP".into(),
            per_base_micros: 853_800,
            as_of: 1_789_776_000,
            source: "ECB".into(),
        }
    }

    #[test]
    fn an_amount_comes_back_from_the_books() -> Result<(), RateError> {
        let rate = pounds();
        // 127,66 € at 0,8538 → 108,99… £: the way back rounds the same way.
        assert_eq!(
            rate.from_base(&Money::eur(12_766))?,
            Money::new(10_900, "GBP")
        );
        assert_eq!(
            rate.from_base(&Money::eur(-12_766))?,
            Money::new(-10_900, "GBP")
        );
        assert_eq!(rate.from_base(&Money::eur(0))?, Money::new(0, "GBP"));
        // It takes the books' currency, not the sale's.
        assert!(matches!(
            rate.from_base(&Money::new(10_900, "GBP")),
            Err(RateError::OtherCurrency { .. })
        ));
        Ok(())
    }

    #[test]
    fn an_amount_goes_to_the_books_at_the_pinned_rate() -> Result<(), RateError> {
        let rate = pounds();
        // 109,00 £ at 0,8538 → 127,664… €.
        assert_eq!(
            rate.to_base(&Money::new(10_900, "GBP"))?,
            Money::eur(12_766)
        );
        // Half a cent rounds up, a credit is a negative amount, zero is zero.
        assert_eq!(
            rate.to_base(&Money::new(-10_900, "GBP"))?,
            Money::eur(-12_766)
        );
        assert_eq!(rate.to_base(&Money::new(0, "GBP"))?, Money::eur(0));
        let parity = PinnedRate {
            per_base_micros: 2_000_000,
            ..pounds()
        };
        assert_eq!(parity.to_base(&Money::new(1, "GBP"))?, Money::eur(1));
        assert_eq!(parity.to_base(&Money::new(3, "GBP"))?, Money::eur(2));
        // Francs are not pounds.
        assert!(matches!(
            rate.to_base(&Money::new(100, "CHF")),
            Err(RateError::OtherCurrency { .. })
        ));
        assert_eq!(rate.quote(), "1 EUR = 0,8538 GBP");
        assert_eq!(
            PinnedRate {
                per_base_micros: 1_085_250,
                ..pounds()
            }
            .quote(),
            "1 EUR = 1,08525 GBP"
        );
        assert_eq!(
            PinnedRate {
                per_base_micros: 25_000_000,
                ..pounds()
            }
            .quote(),
            "1 EUR = 25,0000 GBP"
        );
        Ok(())
    }
}
