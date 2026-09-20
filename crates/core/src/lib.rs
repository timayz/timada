//! Shared kernel for the timada bounded contexts.
//!
//! Only stable, semantics-free concepts live here: money, the currencies a
//! shop sells in, postal addresses,
//! deterministic aggregate ids, URL slugs and clock access. Everything with a lifecycle
//! belongs to the context that owns it.

pub mod address;
pub mod currency;
pub mod format;
pub mod id;
pub mod money;
pub mod slug;
#[cfg(feature = "test-support")]
pub mod testing;
pub mod time;

pub use address::{Address, AddressError, Civility};
pub use currency::{CurrencyError, PerCurrency, ShopCurrencies};
pub use money::{Money, MoneyError};
