//! `GET /store/shoppers/me` with `X-Shopper-Token`: who the token signs in.
//! 200 `{"id", "email", "registered_at", "addresses": [{"id", "saved_at",
//! "address": {name, line1, line2, postal_code, city, country, phone}}]}`,
//! addresses most recently saved first; times are unix seconds. The store
//! only holds a shopper's personal data sealed, so a storefront asks here
//! rather than reading it.

use serde_json::json;
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, route},
};

use super::{Answer, SIGNED_OUT, answer, opened, refused, signed_in, status};

#[route(GET)]
pub async fn me(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let store = super::super::store(cx, &access);
    let Some(shopper) = signed_in(cx, &store).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let Some(shopper) = opened(cx, &access.tenant.id, &shopper).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    let addresses: Vec<_> = shopper
        .addresses
        .iter()
        .map(|saved| {
            let address = &saved.address;
            json!({
                "id": saved.id,
                "saved_at": saved.saved_at,
                "address": {
                    "name": address.name,
                    "line1": address.line1,
                    "line2": address.line2,
                    "postal_code": address.postal_code,
                    "city": address.city,
                    "country": address.country,
                    "phone": address.phone,
                },
            })
        })
        .collect();
    answer(
        StatusCode::OK,
        json!({
            "id": shopper.id,
            "email": shopper.email,
            "registered_at": shopper.registered_at,
            "addresses": addresses,
        }),
    )
}
