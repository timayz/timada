//! One way a product varies.

pub mod kind;
pub mod labels;
pub mod remove;
pub mod rename;
pub mod values;
pub mod visibility;

use topcoat::router::module_param;

module_param!(pub option_id);

#[derive(serde::Deserialize)]
pub struct KindForm {
    /// A key of `OPTION_KINDS`, or empty.
    #[serde(default)]
    pub kind: String,
}
