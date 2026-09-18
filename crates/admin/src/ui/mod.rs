//! Presentation helpers shared by the admin pages.

mod document;
mod format;

pub use document::{empty_state, page_header, pagination, shell};
pub use format::{date, money, order_status_badge};
