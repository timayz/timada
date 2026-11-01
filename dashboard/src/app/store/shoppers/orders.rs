//! `GET /store/shoppers/orders` with `X-Shopper-Token`: the latest orders
//! placed with the shopper's address, guest ones included, newest first.
//! 200 `{"orders": [{id, reference, status, item_count, total_cents,
//! currency, placed_at}]}`. Read from the orders list, so an order paid a
//! moment ago may be missing for a second.

use serde_json::json;
use timada_orders::orders_of_email;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, route},
};

use super::{Answer, SIGNED_OUT, answer, opened, refused, signed_in, status};
use crate::db::Db;

/// The most orders a history lists.
const HISTORY_LEN: i64 = 50;

#[route(GET)]
pub async fn orders(cx: &Cx) -> Result<Answer> {
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
    let rows = orders_of_email(
        app_context::<Db>(cx),
        &access.tenant.id,
        &shopper.email,
        HISTORY_LEN,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let orders: Vec<_> = rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "reference": row.reference,
                "status": row.status,
                "item_count": row.item_count,
                "total_cents": row.total_cents,
                "currency": row.currency,
                "placed_at": row.placed_at,
            })
        })
        .collect();
    answer(StatusCode::OK, json!({ "orders": orders }))
}
