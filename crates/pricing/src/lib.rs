//! Pricing bounded context: what a product *costs* — tax-inclusive price, VAT
//! rate, éco-participation and the "payez en 3x" installment offer shown on
//! the product page. The actual installment plan lives in `timada-payment`.

pub mod aggregator;
mod command;
mod error;
mod query;
mod value_object;

pub use command::*;
pub use error::*;
pub use query::*;
pub use value_object::*;
