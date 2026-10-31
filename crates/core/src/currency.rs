//! The currencies a shop prices in.

/// ISO-4217 codes, uppercase.
pub const CURRENCIES: &[&str] = &["EUR", "USD", "GBP", "CHF"];

/// Uppercases a code and checks it is one of [`CURRENCIES`]; `None`
/// otherwise.
pub fn normalize_currency(currency: &str) -> Option<String> {
    let currency = currency.trim().to_ascii_uppercase();
    CURRENCIES.contains(&currency.as_str()).then_some(currency)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_currency_is_one_the_shops_price_in() {
        assert_eq!(normalize_currency(" eur ").as_deref(), Some("EUR"));
        assert_eq!(normalize_currency("JPY"), None);
        assert_eq!(normalize_currency(""), None);
    }
}
