//! The currencies a shop sells in. A host value: the first one is the shop's
//! **base** currency — the one products are listed in first, and the one the
//! books are kept in — the others are the currencies an operator may also
//! give a product a price in. Nothing is ever converted at checkout: a
//! product without a price in a currency is simply not sold in it.

use crate::Money;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CurrencyError {
    #[error("`{0}` is not an ISO 4217 currency code (three upper-case letters)")]
    Malformed(String),
    /// Amounts are kept in hundredths: a currency without cents (JPY), or
    /// with thousandths (KWD), would be priced a hundred times off.
    #[error("`{0}` does not count in hundredths: not supported")]
    MinorUnit(String),
    #[error("`{0}` is listed twice")]
    Duplicate(String),
}

/// ISO 4217 codes whose minor unit is not a hundredth.
const NOT_IN_HUNDREDTHS: [&str; 25] = [
    "BIF", "CLP", "DJF", "GNF", "ISK", "JPY", "KMF", "KRW", "PYG", "RWF", "UGX", "UYI", "VND",
    "VUV", "XAF", "XOF", "XPF", "BHD", "IQD", "JOD", "KWD", "LYD", "OMR", "TND", "CLF",
];

/// Whether `code` looks like an ISO 4217 code: three upper-case letters.
pub fn is_currency_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShopCurrencies {
    base: String,
    others: Vec<String>,
}

impl Default for ShopCurrencies {
    /// Euros, and nothing else.
    fn default() -> Self {
        Self {
            base: Money::EUR.to_owned(),
            others: Vec::new(),
        }
    }
}

impl ShopCurrencies {
    /// `base` first, then the other currencies the shop sells in, in the
    /// order a switcher should offer them.
    pub fn new(base: &str, others: &[&str]) -> Result<Self, CurrencyError> {
        let mut seen: Vec<String> = Vec::with_capacity(others.len() + 1);
        for code in std::iter::once(&base).chain(others) {
            if !is_currency_code(code) {
                return Err(CurrencyError::Malformed((*code).to_owned()));
            }
            if NOT_IN_HUNDREDTHS.contains(code) {
                return Err(CurrencyError::MinorUnit((*code).to_owned()));
            }
            if seen.iter().any(|known| known == code) {
                return Err(CurrencyError::Duplicate((*code).to_owned()));
            }
            seen.push((*code).to_owned());
        }
        let base = seen.remove(0);
        Ok(Self { base, others: seen })
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// The currencies next to the base one.
    pub fn others(&self) -> &[String] {
        &self.others
    }

    /// Every currency, the base one first.
    pub fn all(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.base.as_str()).chain(self.others.iter().map(String::as_str))
    }

    pub fn sells_in(&self, code: &str) -> bool {
        self.all().any(|known| known == code)
    }

    /// `code` when the shop sells in it, the base currency otherwise: what to
    /// make of a currency read from a cookie or a query string.
    pub fn or_base<'a>(&'a self, code: Option<&str>) -> &'a str {
        code.and_then(|code| self.all().find(|known| *known == code))
            .unwrap_or(&self.base)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shop_names_its_currencies_base_first() -> Result<(), CurrencyError> {
        let shop = ShopCurrencies::new("EUR", &["GBP", "CHF"])?;
        assert_eq!(shop.base(), "EUR");
        assert_eq!(shop.all().collect::<Vec<_>>(), ["EUR", "GBP", "CHF"]);
        assert!(shop.sells_in("GBP") && !shop.sells_in("USD"));
        assert_eq!(shop.or_base(Some("CHF")), "CHF");
        assert_eq!(shop.or_base(Some("USD")), "EUR");
        assert_eq!(shop.or_base(None), "EUR");
        assert_eq!(ShopCurrencies::default().all().collect::<Vec<_>>(), ["EUR"]);

        assert_eq!(
            ShopCurrencies::new("eur", &[]),
            Err(CurrencyError::Malformed("eur".into()))
        );
        assert_eq!(
            ShopCurrencies::new("EUR", &["EUR"]),
            Err(CurrencyError::Duplicate("EUR".into()))
        );
        // A yen has no cents: 1000 "minor units" would be read as ¥10.
        assert_eq!(
            ShopCurrencies::new("EUR", &["JPY"]),
            Err(CurrencyError::MinorUnit("JPY".into()))
        );
        Ok(())
    }
}
