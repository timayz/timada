//! `/checkout/pay`: where the card form used to be, before it moved onto
//! `/checkout`. A bookmark or a stale return from Stripe lands there.

use topcoat::{
    Result,
    context::Cx,
    router::{error::redirect, page, query_params},
    view::View,
};

use crate::app::checkout::checkout_url;

#[topcoat::router::query_params(error = bad_request)]
struct PayQuery {
    error: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<PayQuery>(cx)?;
    Err::<(), _>(redirect(checkout_url(cx, query.error.as_deref())).into())
}
