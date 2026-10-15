//! `POST /store/shoppers/addresses/remove` with `X-Shopper-Token` and
//! `address_id=…`: forgets a saved address. 204, whether or not it was kept.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::super::{Answer, SIGNED_OUT, answer_error, refused, signed_in, status};

#[derive(Deserialize)]
pub struct RemoveForm {
    address_id: String,
}

#[route(POST)]
pub async fn remove(cx: &Cx, Form(form): Form<RemoveForm>) -> Result<Answer> {
    let access = match super::super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = super::super::super::hold(cx, &access).await?;
    let Some(shopper) = signed_in(cx, hold.executor()).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    match timada_shoppers::Command(hold.executor().clone())
        .remove_address(&shopper.id, &form.address_id)
        .await
    {
        Ok(()) => status(StatusCode::NO_CONTENT),
        Err(err) => answer_error(err),
    }
}
