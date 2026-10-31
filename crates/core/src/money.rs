//! Amounts in minor units, written for a person.

/// Minor units as `12.50`, locale-neutral, without the currency.
pub fn format_amount(cents: u64) -> String {
    format!("{}.{:02}", cents / 100, cents % 100)
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
}
