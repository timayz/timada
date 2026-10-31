use std::fmt;

use timada_catalog::format_money;

use crate::error::DiscountError;

pub const MIN_CODE_LEN: usize = 3;
pub const MAX_CODE_LEN: usize = 32;
/// The most orders a capped code serves: its usage stream holds a few events
/// per use and has to stay well inside a stream's versions.
pub const MAX_USES: u32 = 10_000;
/// 100 %, in basis points.
pub const MAX_PERCENT_BPS: u32 = 10_000;
/// What a discounted order still costs at least: the smallest charge a card
/// payment takes. An order nothing is paid for needs a checkout without
/// payment, which there is not.
pub const MIN_CHARGE_CENTS: u64 = 50;
/// How long a checkout holds a use while the shopper pays.
pub const RESERVATION_SECS: u64 = 30 * 60;
/// The most live offers a shop has: a checkout reads them all each time it
/// prices a cart.
pub const MAX_OFFERS: usize = 20;
/// The longest title an offer shows above its item.
pub const MAX_OFFER_TITLE_LEN: usize = 80;

/// What a discount takes off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscountValue {
    /// Basis points of the items subtotal (`1000` is 10 %).
    Percent(u32),
    /// Minor units of a currency.
    Amount { cents: u64, currency: String },
}

impl Default for DiscountValue {
    fn default() -> Self {
        Self::Percent(0)
    }
}

impl DiscountValue {
    /// What it takes off `subtotal_cents`, before any cap: a percentage
    /// rounds half up. `None` for an amount in another currency.
    pub fn off(&self, subtotal_cents: u64, currency: &str) -> Option<u64> {
        match self {
            Self::Percent(bps) => {
                let off = (u128::from(subtotal_cents) * u128::from(*bps) + 5_000) / 10_000;
                Some(off as u64)
            }
            Self::Amount {
                cents,
                currency: of,
            } => of.eq_ignore_ascii_case(currency).then_some(*cents),
        }
    }
}

impl fmt::Display for DiscountValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Percent(bps) => write!(f, "{} %", format_percent(*bps)),
            Self::Amount { cents, currency } => f.write_str(&format_money(*cents, currency)),
        }
    }
}

/// `1250` → `12.5`, `1000` → `10`.
pub fn format_percent(bps: u32) -> String {
    let whole = bps / 100;
    match bps % 100 {
        0 => whole.to_string(),
        frac if frac % 10 == 0 => format!("{whole}.{}", frac / 10),
        frac => format!("{whole}.{frac:02}"),
    }
}

/// Reads a percentage as a merchant types it (`10`, `12.5`, `7,25`) into
/// basis points: more than 0, at most 100, at most two decimals.
pub fn parse_percent(value: &str) -> Result<u32, DiscountError> {
    let value = value.trim().replace(',', ".");
    let (whole, frac) = value.split_once('.').unwrap_or((&value, ""));
    let digits = |s: &str| !s.is_empty() && s.len() <= 3 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || frac.len() > 2 || !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(DiscountError::InvalidPercent);
    }
    let whole: u32 = whole.parse().map_err(|_| DiscountError::InvalidPercent)?;
    let frac: u32 = format!("{frac:0<2}")
        .parse()
        .map_err(|_| DiscountError::InvalidPercent)?;
    let bps = whole * 100 + frac;
    if bps == 0 || bps > MAX_PERCENT_BPS {
        return Err(DiscountError::InvalidPercent);
    }
    Ok(bps)
}

/// Uppercases and validates a code: `[A-Z0-9_-]`, 3 to 32 characters.
pub fn normalize_code(code: &str) -> Result<String, DiscountError> {
    let code = code.trim().to_ascii_uppercase();
    let valid = (MIN_CODE_LEN..=MAX_CODE_LEN).contains(&code.len())
        && code
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'-' || b == b'_');
    if valid {
        Ok(code)
    } else {
        Err(DiscountError::InvalidCode)
    }
}

/// Checks a currency is one the shop prices in.
pub fn normalize_currency(currency: &str) -> Result<String, DiscountError> {
    timada_core::normalize_currency(currency).ok_or(DiscountError::InvalidCurrency)
}

/// The index stream of a code.
pub fn discount_code_id(code: &str) -> String {
    evento::hash_ids(vec!["discount-code", code])
}

/// The usage stream of a capped discount.
pub fn discount_usage_id(discount_id: &str) -> String {
    evento::hash_ids(vec!["discount-usage", discount_id])
}

/// The stream of one shopper's use of a discount good once per e-mail. The
/// e-mail is the normalized one the checkout carries.
pub fn discount_buyer_id(discount_id: &str, email: &str) -> String {
    evento::hash_ids(vec!["discount-buyer", discount_id, email])
}

/// The stream of the tenant's checkout offers: one per tenant store.
pub fn offers_id() -> String {
    evento::hash_ids(vec!["offers"])
}

pub use timada_core::unix_now;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_read_and_print_back() {
        assert_eq!(parse_percent("10").unwrap(), 1000);
        assert_eq!(parse_percent(" 12.5 ").unwrap(), 1250);
        assert_eq!(parse_percent("7,25").unwrap(), 725);
        assert_eq!(parse_percent("100").unwrap(), 10_000);
        for bad in ["0", "100.01", "", "abc", "1.234", "-5", "1000"] {
            assert!(parse_percent(bad).is_err(), "{bad}");
        }
        assert_eq!(format_percent(1000), "10");
        assert_eq!(format_percent(1250), "12.5");
        assert_eq!(format_percent(725), "7.25");
    }

    #[test]
    fn codes_are_uppercased_and_checked() {
        assert_eq!(normalize_code(" summer-24 ").unwrap(), "SUMMER-24");
        assert_eq!(normalize_code("a_b").unwrap(), "A_B");
        for bad in ["ab", "with space", "été", &"X".repeat(33)] {
            assert!(normalize_code(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_percentage_rounds_half_up_and_an_amount_keeps_its_currency() {
        assert_eq!(DiscountValue::Percent(1000).off(1995, "EUR"), Some(200));
        assert_eq!(DiscountValue::Percent(1000).off(1994, "EUR"), Some(199));
        let five = DiscountValue::Amount {
            cents: 500,
            currency: "EUR".to_owned(),
        };
        assert_eq!(five.off(1000, "EUR"), Some(500));
        assert_eq!(five.off(1000, "USD"), None);
    }
}
