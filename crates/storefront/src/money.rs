//! How the shop writes an amount of money.

use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

pub use timada_catalog::format_money;

type Formatter = dyn Fn(u64, &str) -> String + Send + Sync;

/// What is written where shipping costs nothing, unless the host says
/// otherwise.
pub const DEFAULT_FREE: &str = "Free";

/// How an amount of money is written wherever a person reads one: a cart's
/// totals, what a discount takes off, a shipping price, an offer's price,
/// the VAT a total includes, an order's total. The default writes
/// `12.50 EUR` ([`format_money`]); a shop hands its own through
/// [`StoreConfig::money`](crate::StoreConfig::money) so the library writes
/// prices the way its own pages do (`12,50€`). Shipping that costs nothing
/// is a word rather than an amount: [`free`](Self::free) sets it.
///
/// Every view the library builds carries the store's format, so a page
/// never formats a total itself; code outside a request reads it with
/// [`Store::money`](crate::Store::money).
#[derive(Clone)]
pub struct MoneyFormat {
    format: Format,
    free: Cow<'static, str>,
}

#[derive(Clone)]
enum Format {
    Default,
    Custom(Arc<Formatter>),
}

impl MoneyFormat {
    /// Writes amounts with `format`, given the cents and the ISO 4217 code
    /// (`EUR`).
    pub fn new(format: impl Fn(u64, &str) -> String + Send + Sync + 'static) -> Self {
        Self {
            format: Format::Custom(Arc::new(format)),
            free: Cow::Borrowed(DEFAULT_FREE),
        }
    }

    /// The word written where shipping costs nothing (`Free`).
    pub fn free(mut self, word: impl Into<Cow<'static, str>>) -> Self {
        self.free = word.into();
        self
    }

    /// `cents` of `currency` as a person reads them.
    pub fn format(&self, cents: u64, currency: &str) -> String {
        match &self.format {
            Format::Default => format_money(cents, currency),
            Format::Custom(format) => format(cents, currency),
        }
    }

    /// What is written where shipping costs nothing.
    pub fn free_word(&self) -> &str {
        &self.free
    }
}

impl Default for MoneyFormat {
    fn default() -> Self {
        Self {
            format: Format::Default,
            free: Cow::Borrowed(DEFAULT_FREE),
        }
    }
}

impl fmt::Debug for MoneyFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MoneyFormat")
            .field("sample", &self.format(1250, "EUR"))
            .field("free", &self.free)
            .finish()
    }
}

/// Two formats are the same when they are the default, or the very same
/// closure: a view's equality is about what it shows, and the store hands
/// every view one clone of its format.
impl PartialEq for MoneyFormat {
    fn eq(&self, other: &Self) -> bool {
        let same_format = match (&self.format, &other.format) {
            (Format::Default, Format::Default) => true,
            (Format::Custom(mine), Format::Custom(theirs)) => Arc::ptr_eq(mine, theirs),
            _ => false,
        };
        same_format && self.free == other.free
    }
}

impl Eq for MoneyFormat {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_writes_the_locale_neutral_form() {
        let money = MoneyFormat::default();
        assert_eq!(money.format(1250, "EUR"), "12.50 EUR");
        assert_eq!(money.free_word(), "Free");
        assert_eq!(money, MoneyFormat::default());
        assert!(format!("{money:?}").contains("12.50 EUR"));
    }

    #[test]
    fn a_shop_writes_amounts_its_own_way() {
        let money = MoneyFormat::new(|cents, currency| match currency {
            "EUR" => format!("{},{:02}€", cents / 100, cents % 100),
            other => format!("{},{:02} {other}", cents / 100, cents % 100),
        })
        .free("Offerte");
        assert_eq!(money.format(1250, "EUR"), "12,50€");
        assert_eq!(money.format(5, "USD"), "0,05 USD");
        assert_eq!(money.free_word(), "Offerte");
        assert_eq!(money.clone(), money, "a clone is the same format");
        assert_ne!(money, MoneyFormat::default());
        assert_ne!(
            money,
            MoneyFormat::new(|cents, _| cents.to_string()).free("Offerte"),
            "another closure is another format"
        );
    }
}
