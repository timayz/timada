//! How prices read on the pages: `12.50 EUR` unless the shop says otherwise
//! ([`MoneyFormat::french`] writes `19,90€`). The view models format every
//! amount through the shop's [`MoneyFormat`], and the scripts get it as a
//! `data-money` descriptor so what they compute reads the same.

use crate::query::Product;

/// How an amount of money is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoneyFormat {
    /// Between the units and the cents: `.` or `,`.
    pub decimal: char,
    /// Between groups of three digits (`1 234`), if any.
    pub group: Option<char>,
    /// The symbol a currency is written with (`EUR` → `€`); a currency not
    /// listed keeps its code.
    pub symbols: Vec<(String, String)>,
    /// Whether the unit comes before the amount (`$12.50`).
    pub unit_first: bool,
    /// What separates the amount from a symbol (`""` for `19,90€`).
    pub symbol_gap: String,
    /// What separates the amount from a code (`" "` for `12.50 EUR`).
    pub code_gap: String,
    /// Between the two ends of a range.
    pub range_separator: String,
    /// Whether both ends of a range carry the unit (`19,90€ – 29,90€`) or
    /// only the upper one (`12.00–18.00 EUR`).
    pub range_repeats_unit: bool,
}

impl Default for MoneyFormat {
    fn default() -> Self {
        Self::international()
    }
}

impl MoneyFormat {
    /// `12.50 EUR`, `12.00–18.00 EUR`: what
    /// [`format_money`](timada_catalog::format_money) writes.
    pub fn international() -> Self {
        Self {
            decimal: '.',
            group: None,
            symbols: Vec::new(),
            unit_first: false,
            symbol_gap: String::new(),
            code_gap: " ".to_owned(),
            range_separator: "–".to_owned(),
            range_repeats_unit: false,
        }
    }

    /// `19,90€`, `1 234,50 USD` (a no-break space), `19,90€ – 29,90€`.
    pub fn french() -> Self {
        Self {
            decimal: ',',
            group: Some('\u{a0}'),
            symbols: vec![("EUR".to_owned(), "€".to_owned())],
            unit_first: false,
            symbol_gap: String::new(),
            code_gap: "\u{a0}".to_owned(),
            range_separator: " – ".to_owned(),
            range_repeats_unit: true,
        }
    }

    /// The symbol `currency` is written with, if it has one here.
    pub fn symbol(&self, currency: &str) -> Option<&str> {
        self.symbols
            .iter()
            .find(|(code, _)| code.eq_ignore_ascii_case(currency))
            .map(|(_, symbol)| symbol.as_str())
    }

    /// The amount alone: `1 234,50`.
    pub fn amount(&self, cents: u64) -> String {
        let whole = (cents / 100).to_string();
        let mut grouped = String::new();
        for (i, digit) in whole.chars().enumerate() {
            if i > 0
                && let Some(group) = self.group
                && (whole.len() - i).is_multiple_of(3)
            {
                grouped.push(group);
            }
            grouped.push(digit);
        }
        format!("{grouped}{}{:02}", self.decimal, cents % 100)
    }

    /// The unit as written, and what separates it from the amount.
    fn unit(&self, currency: &str) -> (String, &str) {
        match self.symbol(currency) {
            Some(symbol) => (symbol.to_owned(), self.symbol_gap.as_str()),
            None => (currency.to_uppercase(), self.code_gap.as_str()),
        }
    }

    /// `19,90€`.
    pub fn format(&self, cents: u64, currency: &str) -> String {
        let (unit, gap) = self.unit(currency);
        let amount = self.amount(cents);
        if self.unit_first {
            format!("{unit}{gap}{amount}")
        } else {
            format!("{amount}{gap}{unit}")
        }
    }

    /// `-2,50€` for what comes off, `2,50€` otherwise.
    pub fn signed(&self, cents: i64, currency: &str) -> String {
        let formatted = self.format(cents.unsigned_abs(), currency);
        if cents < 0 {
            format!("-{formatted}")
        } else {
            formatted
        }
    }

    /// `12.00–18.00 EUR`, or the one amount when both ends are equal.
    pub fn range(&self, min_cents: u64, max_cents: u64, currency: &str) -> String {
        if min_cents == max_cents {
            return self.format(max_cents, currency);
        }
        let max = self.format(max_cents, currency);
        let min = if self.range_repeats_unit {
            self.format(min_cents, currency)
        } else {
            self.amount(min_cents)
        };
        format!("{min}{}{max}", self.range_separator)
    }

    /// A product's price line: one amount, or a range across its variants;
    /// empty when it has no price.
    pub fn price_line(&self, product: &Product) -> String {
        match product.price_range() {
            Some(range) => self.range(
                range.min_cents.max(0) as u64,
                range.max_cents.max(0) as u64,
                &range.currency,
            ),
            None => String::new(),
        }
    }

    /// What a page's script formats amounts with, as JSON for a `data-money`
    /// attribute: `{"decimal":",","group":" ","unit":"€","unitFirst":false,"gap":""}`.
    pub fn descriptor(&self, currency: &str) -> String {
        let (unit, gap) = self.unit(currency);
        serde_json::json!({
            "decimal": self.decimal,
            "group": self.group.map(String::from).unwrap_or_default(),
            "unit": unit,
            "unitFirst": self.unit_first,
            "gap": gap,
        })
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn international_matches_the_catalog_format() {
        let format = MoneyFormat::international();
        assert_eq!(format.format(1250, "EUR"), "12.50 EUR");
        assert_eq!(
            format.format(1250, "EUR"),
            timada_catalog::format_money(1250, "EUR")
        );
        assert_eq!(format.format(12_345_678, "USD"), "123456.78 USD");
        assert_eq!(format.range(1200, 1800, "EUR"), "12.00–18.00 EUR");
        assert_eq!(format.range(1200, 1200, "EUR"), "12.00 EUR");
        assert_eq!(format.signed(-250, "EUR"), "-2.50 EUR");
    }

    #[test]
    fn french_writes_the_symbol_and_groups_thousands() {
        let format = MoneyFormat::french();
        assert_eq!(format.format(1990, "EUR"), "19,90€");
        assert_eq!(format.format(123_450, "USD"), "1\u{a0}234,50\u{a0}USD");
        assert_eq!(format.format(0, "EUR"), "0,00€");
        assert_eq!(format.range(1990, 2990, "EUR"), "19,90€ – 29,90€");
        assert_eq!(
            format.descriptor("EUR"),
            r#"{"decimal":",","gap":"","group":" ","unit":"€","unitFirst":false}"#
        );
    }

    #[test]
    fn unit_first_puts_the_symbol_before() {
        let format = MoneyFormat {
            unit_first: true,
            symbols: vec![("USD".to_owned(), "$".to_owned())],
            ..MoneyFormat::international()
        };
        assert_eq!(format.format(1250, "USD"), "$12.50");
        assert_eq!(format.format(1250, "EUR"), "EUR 12.50");
    }
}
