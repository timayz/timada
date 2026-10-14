use timada_storefront::set_cart_quantity;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{LineForm, cart_url};
use crate::shopper::cart_id;

/// Sets a line's quantity; zero takes it out.
#[route(POST)]
pub async fn quantity(cx: &Cx, Form(form): Form<LineForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(cart_url(cx, None)));
    };
    let quantity = form.quantity.unwrap_or(0);
    let outcome =
        set_cart_quantity(cx, &cart_id, &form.product_id, form.variant_id(), quantity).await?;
    let error = outcome.err().and_then(|err| err.code());
    Ok(see_other(cart_url(cx, error)))
}
