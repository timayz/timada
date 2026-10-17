//! `POST /store/shoppers/addresses` with `X-Shopper-Token` and the fields of
//! an address (`name`, `line1`, `line2`, `postal_code`, `city`, `country`,
//! `phone`): keeps it on the account. 201 `{"id": …}`; saving one already
//! kept answers its id again.

pub mod remove;

use serde::Deserialize;
use serde_json::json;
use timada_orders::ShippingAddress;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Form, route},
};

use super::{Answer, SIGNED_OUT, answer, answer_error, refused, signed_in, status};

#[derive(Deserialize)]
pub struct AddressForm {
    name: String,
    line1: String,
    #[serde(default)]
    line2: Option<String>,
    postal_code: String,
    city: String,
    country: String,
    #[serde(default)]
    phone: Option<String>,
}

#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<AddressForm>) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = super::super::hold(cx, &access).await?;
    let Some(shopper) = signed_in(cx, hold.executor()).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let address = ShippingAddress {
        name: form.name,
        line1: form.line1,
        line2: form.line2,
        postal_code: form.postal_code,
        city: form.city,
        country: form.country,
        phone: form.phone,
    };
    match timada_shoppers::Command(hold.executor().clone())
        .save_address(&*super::keys(cx, &access.tenant.id), &shopper.id, address)
        .await
    {
        Ok(id) => answer(StatusCode::CREATED, json!({ "id": id })),
        Err(err) => answer_error(err),
    }
}
