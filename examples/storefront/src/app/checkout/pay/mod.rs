//! `/checkout/pay`: the card form. Stripe's Payment Element is mounted with
//! the payment the dashboard opened; the browser confirms it with Stripe and
//! comes back to `/checkout/complete`. The card never touches this app.

use timada_storefront::{categories, format_money, order_of_cart, payment_session};
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
use crate::app::{document, say};
use crate::shopper::payment_in_progress;

/// Mounts the Payment Element and confirms the payment on submit. Static and
/// trusted: everything it needs comes from the form's `data-` attributes.
const PAY_SCRIPT: &str = r##"
const form = document.querySelector("[data-payment-form]");
const stripe = Stripe(form.dataset.publishableKey);
const elements = stripe.elements({ clientSecret: form.dataset.clientSecret });
elements.create("payment").mount("#payment-element");
form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const button = form.querySelector("button");
  button.disabled = true;
  const { error } = await stripe.confirmPayment({
    elements,
    confirmParams: { return_url: new URL(form.dataset.returnPath, location.href).href },
  });
  // Only reached when the payment could not be confirmed; otherwise the
  // browser is on its way to the return page.
  document.querySelector("[data-payment-error]").textContent = error.message;
  button.disabled = false;
});
"##;

#[topcoat::router::query_params(error = bad_request)]
struct PayQuery {
    error: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<PayQuery>(cx)?;
    let expired = checkout_url(cx, Some("payment_expired"));
    let progress = payment_in_progress(cx).ok_or_redirect(&expired)?;
    // Paid already (a second tab, a back button): straight to the order.
    if order_of_cart(cx, &progress.cart_id).await?.is_some() {
        return Err(redirect(href!(super::complete::index).resolve(cx)).into());
    }
    let session = payment_session(cx, &progress.cart_id, &progress.request_id)
        .await?
        .ok_or_redirect(&expired)?;
    let nav = categories(cx).await?;
    let return_path = href!(super::complete::index).resolve(cx);
    let total = session.total();
    let currency = session.currency.clone();

    Ok(view! {
        document(
            title: "Payment",
            nav: &nav,
            <h1>"Payment"</h1>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            <table class="cart" data-payment-summary="">
                <tbody>
                    for line in &session.lines {
                        <tr>
                            <td>
                                (&line.name)
                                if let Some(label) = &line.variant_label { " · " (label) }
                            </td>
                            <td>"× " (line.quantity)</td>
                            <td>(format_money(line.amount_cents(), &currency))</td>
                        </tr>
                    }
                </tbody>
            </table>
            <p>"Total: " <strong class="price" data-payment-total="">(&total)</strong></p>
            <form data-payment-form="" data-publishable-key=(&session.publishable_key)
                data-client-secret=(&session.client_secret) data-return-path=(&return_path)>
                <div id="payment-element"></div>
                <p class="notice" role="alert" data-payment-error=""></p>
                <button class="primary">"Pay " (&total)</button>
            </form>
            <p><a href=(href!(crate::app::cart::index))>"Back to the cart"</a></p>
            <script src="https://js.stripe.com/v3/"></script>
            <script>(Unescaped::new_unchecked(PAY_SCRIPT))</script>
        )
    })
}
