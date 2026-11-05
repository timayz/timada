//! `POST /account/addresses/remove`: forgets a saved address, then back to
//! the account.

use timada_storefront::remove_address;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::account::account_url;
use crate::shopper::{forget_shopper, shopper_token};

#[derive(serde::Deserialize)]
pub struct RemoveForm {
    address_id: String,
}

#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<SeeOther> {
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(account_url(cx, None)));
    };
    let error = match remove_address(cx, &token, &form.address_id).await? {
        Ok(()) => None,
        Err(refusal) => {
            if refusal.is_signed_out() {
                forget_shopper(cx);
            }
            Some(refusal.code())
        }
    };
    Ok(see_other(account_url(cx, error)))
}
