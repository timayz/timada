//! `/reviews/{token}`: the link a review request mails the buyer of a
//! fulfilled order. A valid token stands in for the browser that placed the
//! order: the order is remembered here as if this browser had placed it, and
//! the shopper lands on its review forms. A stale or unknown token says so
//! and offers to sign in instead.

use timada_storefront::{menu, review_link};
use topcoat::{
    Result,
    context::Cx,
    router::{error::redirect, href, module_param, page},
    view::{View, view},
};

use crate::app::account::sign_in::sign_in_url;
use crate::app::document;
use crate::app::orders::order_id::{index as order_page, order_id};
use crate::app::words::t;
use crate::shopper::remember_order;

module_param!(pub token);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let token = token(cx);
    if let Some(order_id) = review_link(cx, token).await? {
        remember_order(cx, &order_id)?;
        let to = format!(
            "{}#reviews",
            href!(order_page, order_id: order_id).resolve(cx)
        );
        return Err(redirect(to).into());
    }
    let nav = menu(cx).await?;
    let title = t!(order_review_link_expired_title).resolve(cx);

    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>(t!(order_review_link_expired_heading))</h1>
                <p data-review-link-expired="">
                    (t!(order_review_link_expired_text))
                </p>
                <p><a class="primary" href=(sign_in_url(cx, None, None))>(t!(shell_sign_in))</a></p>
            </div>
        )
    })
}
