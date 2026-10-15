//! `/checkout/complete`: where Stripe sends the browser back. The order
//! exists once Stripe told the dashboard the payment went through, which may
//! take a moment longer than the browser's way back: until then the page
//! says so and looks again.

use timada_storefront::{categories, order_of_cart};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{RouterErrorExt, redirect},
        href, page, query_params,
    },
    view::{Unescaped, View, view},
};

use crate::app::checkout::checkout_url;
use crate::app::document;
use crate::shopper::{
    checkout_draft, forget_cart, forget_payment, payment_in_progress, remember_order,
    save_checkout_draft,
};

/// Looks again in two seconds.
const RELOAD_SCRIPT: &str = "setTimeout(() => location.reload(), 2000);";

#[topcoat::router::query_params(error = bad_request)]
struct CompleteQuery {
    /// What Stripe says about the confirmation: `succeeded`, `processing`
    /// or `failed`.
    redirect_status: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CompleteQuery>(cx)?;
    let progress =
        payment_in_progress(cx).ok_or_redirect(checkout_url(cx, Some("payment_expired")))?;
    if query.redirect_status.as_deref() == Some("failed") {
        return Err(redirect(checkout_url(cx, Some("payment_failed"))).into());
    }
    if let Some(order) = order_of_cart(cx, &progress.cart_id).await? {
        remember_order(cx, &order.id)?;
        forget_cart(cx);
        forget_payment(cx);
        // The address stays for the next order; the note was for this one.
        let mut draft = checkout_draft(cx);
        draft.note.clear();
        save_checkout_draft(cx, draft)?;
        let to = href!(
            crate::app::orders::order_id::index,
            crate::app::orders::order_id::OrderId(order.id)
        )
        .resolve(cx);
        return Err(redirect(to).into());
    }
    let nav = categories(cx).await?;

    Ok(view! {
        document(
            title: "Confirming your payment",
            nav: &nav,
            <h1>"Confirming your payment…"</h1>
            <p data-payment-confirming="">"This takes a few seconds. The page looks again on its own."</p>
            <p><a href=(href!(index))>"Check again"</a></p>
            <script>(Unescaped::new_unchecked(RELOAD_SCRIPT))</script>
        )
    })
}
