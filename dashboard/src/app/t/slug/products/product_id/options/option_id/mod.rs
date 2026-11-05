//! One way a product varies.

pub mod labels;
pub mod remove;
pub mod rename;
pub mod values;
pub mod visibility;

use topcoat::router::module_param;

module_param!(pub option_id);
