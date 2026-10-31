//! Amounts in minor units, written for a person.

/// Minor units as `12.50`, locale-neutral, without the currency.
pub fn format_amount(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
}

/// The sign a currency is written with (`€`), or its code when it has no
/// sign in common use (`CHF`).
pub fn currency_symbol(currency: &str) -> &str {
    match currency {
        "EUR" => "€",
        "USD" => "$",
        "GBP" => "£",
        other => other,
    }
}

/// Minor units with their currency, `12.50 €`, locale-neutral.
pub fn format_money(cents: u64, currency: &str) -> String {
    format!("{} {}", format_amount(cents), currency_symbol(currency))
}

/// Two amounts of one currency as a range, `12.00–18.00 €`.
pub fn format_money_range(min_cents: u64, max_cents: u64, currency: &str) -> String {
    format!(
        "{}–{} {}",
        format_amount(min_cents),
        format_amount(max_cents),
        currency_symbol(currency)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_amount_reads_with_two_decimals() {
        assert_eq!(format_amount(1250), "12.50");
        assert_eq!(format_amount(5), "0.05");
        assert_eq!(format_amount(0), "0.00");
    }

    #[test]
    fn money_is_written_with_its_sign() {
        assert_eq!(format_money(1250, "EUR"), "12.50 €");
        assert_eq!(format_money(1250, "USD"), "12.50 $");
        assert_eq!(format_money(1250, "GBP"), "12.50 £");
        assert_eq!(format_money(1250, "CHF"), "12.50 CHF");
        assert_eq!(format_money_range(1200, 1800, "EUR"), "12.00–18.00 €");
    }
}
