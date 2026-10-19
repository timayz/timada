//! One imported AliExpress product: read it again, give it its own price
//! rule, add or ignore the combinations a read found, or stop keeping it in
//! step. Each action goes back to where it was
//! sent from: the AliExpress page, or the product's page with `back`.

pub mod combinations;
pub mod ignore;
pub mod pricing;
pub mod restore;
pub mod sync;
pub mod unlink;

use topcoat::{
    context::Cx,
    router::{error::SeeOther, error::see_other, module_param},
};

use crate::app::t::slug::TenantAccess;

module_param!(pub link_id);

/// Where an action goes back to: the product's page when `back` names one.
#[derive(Debug, Default, serde::Deserialize)]
pub struct Back {
    #[serde(default)]
    pub back: String,
}

/// A combination found, as the forms send it.
#[derive(Debug, Default, serde::Deserialize)]
pub struct CombinationForm {
    #[serde(default)]
    pub back: String,
    /// Blank: every combination offered.
    #[serde(default)]
    pub sku_id: String,
}

pub fn back_to(
    cx: &Cx,
    access: &TenantAccess,
    back: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> SeeOther {
    let slug = access.tenant.slug.as_str();
    if !back.is_empty() && back.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return see_other(crate::app::t::slug::products::product_id::product_url(
            cx, slug, back, error, message,
        ));
    }
    see_other(super::super::aliexpress_url(cx, slug, error, message))
}
