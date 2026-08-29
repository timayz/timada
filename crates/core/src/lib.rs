//! Timada shared kernel.
//!
//! Value objects ([`Money`], [`Currency`]), ULID id helpers, the framework-wide
//! [`AppError`], SQLite pool builders following the split read/write
//! convention, and the evento [`Executor`] + [`ServiceContext`] every service
//! crate hangs its state on.

pub mod db;
pub mod error;
pub mod executor;
pub mod id;
pub mod money;
pub mod time;

pub use error::{AppError, AppResult};
pub use executor::{Executor, ServiceContext};
pub use id::new_id;
pub use money::{Currency, Money, MoneyError};
pub use time::{format_utc_date, format_utc_datetime, now_millis};
