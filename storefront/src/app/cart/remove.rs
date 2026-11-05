use timada_storefront::remove_from_cart;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{LineForm, back_url, cart_url};
use crate::shopper::cart_id;

/// Takes a line out of the cart.
#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<LineForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(cart_url(cx, None)));
    };
    let outcome = remove_from_cart(cx, &cart_id, &form.product_id, form.variant_id()).await?;
    let error = outcome.err().and_then(|err| err.code());
    Ok(see_other(back_url(cx, &form.back, &form.from, error)))
}
