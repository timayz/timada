use bitcode::{Decode, Encode};

/// An amount in minor units (cents) of an ISO 4217 currency.
#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct Money {
    pub minor: i64,
    pub currency: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MoneyError {
    #[error("currency mismatch: {left} vs {right}")]
    CurrencyMismatch { left: String, right: String },
    #[error("money arithmetic overflow")]
    Overflow,
}

impl Default for Money {
    fn default() -> Self {
        Self::eur(0)
    }
}

impl Money {
    pub const EUR: &'static str = "EUR";

    pub fn new(minor: i64, currency: impl Into<String>) -> Self {
        Self {
            minor,
            currency: currency.into(),
        }
    }

    pub fn eur(minor: i64) -> Self {
        Self::new(minor, Self::EUR)
    }

    pub fn zero(currency: impl Into<String>) -> Self {
        Self::new(0, currency)
    }

    pub fn is_positive(&self) -> bool {
        self.minor > 0
    }

    pub fn is_negative(&self) -> bool {
        self.minor < 0
    }

    pub fn same_currency(&self, other: &Money) -> Result<(), MoneyError> {
        if self.currency == other.currency {
            Ok(())
        } else {
            Err(MoneyError::CurrencyMismatch {
                left: self.currency.clone(),
                right: other.currency.clone(),
            })
        }
    }

    pub fn checked_add(&self, other: &Money) -> Result<Money, MoneyError> {
        self.same_currency(other)?;
        let minor = self
            .minor
            .checked_add(other.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Money::new(minor, &self.currency))
    }

    pub fn checked_sub(&self, other: &Money) -> Result<Money, MoneyError> {
        self.same_currency(other)?;
        let minor = self
            .minor
            .checked_sub(other.minor)
            .ok_or(MoneyError::Overflow)?;
        Ok(Money::new(minor, &self.currency))
    }

    pub fn checked_mul(&self, quantity: u32) -> Result<Money, MoneyError> {
        let minor = self
            .minor
            .checked_mul(i64::from(quantity))
            .ok_or(MoneyError::Overflow)?;
        Ok(Money::new(minor, &self.currency))
    }

    /// Splits the amount into `count` equal parts, rounding down; the
    /// remainder is dropped (display estimate, not an accounting split).
    pub fn divided_by(&self, count: u32) -> Money {
        let divisor = i64::from(count.max(1));
        Money::new(self.minor / divisor, &self.currency)
    }

    /// Extracts the pre-tax amount from a tax-inclusive one, given a VAT rate
    /// in basis points (2000 = 20 %). Rounds to the nearest minor unit.
    pub fn excl_tax(&self, vat_rate_bp: u16) -> Money {
        let rate = 10_000 + i128::from(vat_rate_bp);
        let scaled = i128::from(self.minor) * 10_000;
        let half = rate / 2;
        let minor = (scaled + half) / rate;
        Money::new(minor as i64, &self.currency)
    }

    /// The tax-inclusive amount of a pre-tax one, given a VAT rate in basis
    /// points (2000 = 20 %). The way back from [`Money::excl_tax`]; rounds
    /// to the nearest minor unit, so the round trip can differ by a cent.
    pub fn incl_tax(&self, vat_rate_bp: u16) -> Money {
        let rate = 10_000 + i128::from(vat_rate_bp);
        let scaled = i128::from(self.minor) * rate;
        let minor = (scaled + 5_000) / 10_000;
        Money::new(minor as i64, &self.currency)
    }

    /// Applies a percentage in basis points (1000 = 10 %), rounding to nearest.
    pub fn percent_bp(&self, bp: u16) -> Money {
        let scaled = i128::from(self.minor) * i128::from(bp);
        let minor = (scaled + 5_000) / 10_000;
        Money::new(minor as i64, &self.currency)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_same_currency() -> Result<(), MoneyError> {
        let total = Money::eur(11_995).checked_add(&Money::eur(170))?;
        assert_eq!(total, Money::eur(12_165));
        Ok(())
    }

    #[test]
    fn adds_and_removes_vat() {
        // 53,82 € HT at 20 % is 64,58 € TTC, and back again.
        assert_eq!(Money::eur(5_382).incl_tax(2000), Money::eur(6_458));
        assert_eq!(Money::eur(6_458).excl_tax(2000), Money::eur(5_382));
        assert_eq!(Money::eur(1_000).incl_tax(0), Money::eur(1_000));
    }

    #[test]
    fn rejects_currency_mismatch() {
        let err = Money::eur(1).checked_add(&Money::new(1, "USD"));
        assert_eq!(
            err,
            Err(MoneyError::CurrencyMismatch {
                left: "EUR".into(),
                right: "USD".into()
            })
        );
    }

    #[test]
    fn extracts_pre_tax_amount() {
        assert_eq!(Money::eur(12_000).excl_tax(2_000), Money::eur(10_000));
    }

    #[test]
    fn splits_installments() {
        assert_eq!(Money::eur(12_474).divided_by(3), Money::eur(4_158));
    }

    #[test]
    fn detects_overflow() {
        assert_eq!(
            Money::eur(i64::MAX).checked_mul(2),
            Err(MoneyError::Overflow)
        );
    }
}
