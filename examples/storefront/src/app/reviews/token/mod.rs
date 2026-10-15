//! `/reviews/{token}`: the link a review request mails the buyer of a
//! fulfilled order. A valid token stands in for the browser that placed the
//! order: the order is remembered here as if this browser had placed it, and
//! the shopper lands on its review forms. A stale or unknown token says so
//! and offers to sign in instead.

use timada_storefront::{menu, review_link};
use topcoat::{
    Result,
    context::Cx,
    router::{error::redirect, href, module_param, page, path_param},
    view::{View, view},
};

use crate::app::account::sign_in::sign_in_url;
use crate::app::document;
use crate::app::orders::order_id::{OrderId, index as order_page};
use crate::shopper::remember_order;

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = path_param::<Token>(cx);
    if let Some(order_id) = review_link(cx, token).await? {
        remember_order(cx, &order_id)?;
        let to = format!(
            "{}#reviews",
            href!(order_page, OrderId(order_id)).resolve(cx)
        );
        return Err(redirect(to).into());
    }
    let nav = menu(cx).await?;

    Ok(view! {
        document(
            title: "Link expired",
            nav: &nav,
            <h1>"This link has expired"</h1>
            <p data-review-link-expired="">
                "The link to review your order no longer works. Sign in with the e-mail address you ordered with to find your orders and review them."
            </p>
            <p><a class="primary" href=(sign_in_url(cx, None, None))>"Sign in"</a></p>
        )
    })
}
