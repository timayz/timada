//! Presentation of values for the shop's audience (French): shared by the
//! storefront, the admin and the e-mails so they never disagree.

use crate::Money;

/// `1 658,19 €` — narrow no-break spaces between thousands, comma decimals.
pub fn money(money: &Money) -> String {
    let sign = if money.minor < 0 { "-" } else { "" };
    let abs = money.minor.unsigned_abs();
    let (units, cents) = (abs / 100, abs % 100);
    let digits = units.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push('\u{202f}');
        }
        grouped.push(ch);
    }
    let symbol = match money.currency.as_str() {
        "EUR" => "€".to_owned(),
        "USD" => "$".to_owned(),
        other => other.to_owned(),
    };
    format!("{sign}{grouped},{cents:02} {symbol}")
}

/// `21/11/2024` from Unix seconds (UTC).
pub fn date(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!("{day:02}/{month:02}/{year}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_money_and_dates() {
        assert_eq!(money(&Money::eur(165_819)), "1\u{202f}658,19 €");
        assert_eq!(money(&Money::eur(-5)), "-0,05 €");
        assert_eq!(money(&Money::new(1_000_000, "CHF")), "10\u{202f}000,00 CHF");
        assert_eq!(date(1_732_147_200), "21/11/2024");
    }
}
