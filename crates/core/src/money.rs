use std::fmt;

/// ISO currency of a [`Money`] amount.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub enum Currency {
    #[default]
    Eur,
    Usd,
}

impl Currency {
    pub fn code(&self) -> &'static str {
        match self {
            Currency::Eur => "EUR",
            Currency::Usd => "USD",
        }
    }

    pub fn from_code(code: &str) -> Result<Self, MoneyError> {
        match code {
            "EUR" => Ok(Currency::Eur),
            "USD" => Ok(Currency::Usd),
            other => Err(MoneyError::UnknownCurrency(other.to_owned())),
        }
    }
}

impl fmt::Display for Currency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MoneyError {
    #[error("currency mismatch: {0} vs {1}")]
    CurrencyMismatch(Currency, Currency),
    #[error("unknown currency code: {0}")]
    UnknownCurrency(String),
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.amount_cents < 0 { "-" } else { "" };
        let abs = self.amount_cents.unsigned_abs();
        write!(f, "{sign}{}.{:02} {}", abs / 100, abs % 100, self.currency)
    }
}

/// An exact monetary amount in minor units (cents).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Money {
    pub amount_cents: i64,
    pub currency: Currency,
}

impl Money {
    pub fn new(amount_cents: i64, currency: Currency) -> Self {
        Self {
            amount_cents,
            currency,
        }
    }

    pub fn zero(currency: Currency) -> Self {
        Self::new(0, currency)
    }

    pub fn multiply(&self, quantity: u32) -> Self {
        Self::new(
            self.amount_cents.saturating_mul(i64::from(quantity)),
            self.currency,
        )
    }

    pub fn subtract(&self, other: Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, other.currency));
        }
        Ok(Self::new(
            self.amount_cents.saturating_sub(other.amount_cents),
            self.currency,
        ))
    }

    pub fn add(&self, other: Self) -> Result<Self, MoneyError> {
        if self.currency != other.currency {
            return Err(MoneyError::CurrencyMismatch(self.currency, other.currency));
        }
        Ok(Self::new(
            self.amount_cents.saturating_add(other.amount_cents),
            self.currency,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_minor_units_with_currency_code() {
        assert_eq!(Money::new(1234, Currency::Eur).to_string(), "12.34 EUR");
        assert_eq!(Money::new(5, Currency::Usd).to_string(), "0.05 USD");
        assert_eq!(Money::new(-1234, Currency::Eur).to_string(), "-12.34 EUR");
    }

    #[test]
    fn adds_same_currency_and_rejects_mismatch() {
        let a = Money::new(100, Currency::Eur);
        let b = Money::new(50, Currency::Eur);
        assert_eq!(a.add(b).unwrap(), Money::new(150, Currency::Eur));

        let c = Money::new(50, Currency::Usd);
        assert_eq!(
            a.add(c),
            Err(MoneyError::CurrencyMismatch(Currency::Eur, Currency::Usd))
        );
    }

    #[test]
    fn multiplies_by_quantity() {
        assert_eq!(
            Money::new(199, Currency::Eur).multiply(3),
            Money::new(597, Currency::Eur)
        );
    }
}
