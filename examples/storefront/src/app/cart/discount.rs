use timada_storefront::{apply_discount, remove_discount};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{back_url, cart_url};
use crate::shopper::cart_id;

#[derive(serde::Deserialize)]
pub struct DiscountForm {
    #[serde(default)]
    pub code: String,
    /// Set by the button that takes the code off.
    #[serde(default)]
    pub remove: Option<String>,
    /// `checkout` when sent from the checkout's summary, `drawer` from the
    /// cart drawer.
    #[serde(default)]
    pub back: String,
    /// The page the drawer sat on.
    #[serde(default)]
    pub from: String,
}

/// Puts a discount code on the cart, or takes it off.
#[route(POST)]
pub async fn discount(cx: &Cx, Form(form): Form<DiscountForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(cart_url(cx, None)));
    };
    let outcome = match form.remove {
        Some(_) => remove_discount(cx, &cart_id).await?,
        None => apply_discount(cx, &cart_id, &form.code).await?,
    };
    let error = outcome.err().and_then(|err| err.code());
    Ok(see_other(back_url(cx, &form.back, &form.from, error)))
}
