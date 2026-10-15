//! `/checkout`: the cart's summary and the guest's details (GET), then
//! asking to pay (POST), which leads to the card form at `/checkout/pay`.
//! The total the page showed travels with the form, so a price the shopper
//! did not see is never charged.

pub mod complete;
pub mod pay;

use timada_storefront::{Checkout, OrdersError, categories, format_money, start_payment};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, view},
};

use crate::app::cart::{cart_url, current_cart};
use crate::app::{document, say};
use crate::shopper::{
    CheckoutDraft, PaymentInProgress, cart_id, checkout_draft, save_checkout_draft,
    set_payment_in_progress,
};

#[topcoat::router::query_params(error = bad_request)]
struct CheckoutQuery {
    error: Option<String>,
}

pub fn checkout_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CheckoutQuery>(cx)?;
    // Nothing to order (or something off sale to remove first): the cart.
    let cart = current_cart(cx)
        .await?
        .filter(|cart| cart.can_check_out())
        .ok_or_redirect(cart_url(cx, None))?;
    let nav = categories(cx).await?;
    let draft = checkout_draft(cx);
    let currency = cart.currency.clone().unwrap_or_default();

    Ok(view! {
        document(
            title: "Checkout",
            nav: &nav,
            <h1>"Checkout"</h1>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            <table class="cart" data-checkout-summary="">
                <tbody>
                    for line in &cart.lines {
                        <tr>
                            <td>
                                (&line.name)
                                if let Some(label) = &line.variant_label { " · " (label) }
                            </td>
                            <td>"× " (line.quantity)</td>
                            <td>(format_money(line.amount_cents.unwrap_or_default(), &currency))</td>
                        </tr>
                    }
                </tbody>
            </table>
            <p>"Total: " <strong class="price" data-checkout-total="">(cart.total().unwrap_or_default())</strong></p>

            <form class="checkout" method="post" action=(href!(place))>
                <input type="hidden" name="expected_total_cents" value=(cart.total_cents)>
                <label for="checkout-email">"E-mail"</label>
                <input id="checkout-email" name="email" type="email" required="" autocomplete="email" value=(&draft.email)>
                <label for="checkout-name">"Full name"</label>
                <input id="checkout-name" name="name" required="" autocomplete="name" value=(&draft.name)>
                <label for="checkout-line1">"Street address"</label>
                <input id="checkout-line1" name="line1" required="" autocomplete="address-line1" value=(&draft.line1)>
                <label for="checkout-line2">"Apartment, suite (optional)"</label>
                <input id="checkout-line2" name="line2" autocomplete="address-line2" value=(&draft.line2)>
                <label for="checkout-postal-code">"Postal code"</label>
                <input id="checkout-postal-code" name="postal_code" required="" autocomplete="postal-code" value=(&draft.postal_code)>
                <label for="checkout-city">"City"</label>
                <input id="checkout-city" name="city" required="" autocomplete="address-level2" value=(&draft.city)>
                <label for="checkout-country">"Country code"</label>
                <input id="checkout-country" name="country" required="" maxlength="2" autocomplete="country" placeholder="FR" value=(&draft.country)>
                <label for="checkout-phone">"Phone (optional)"</label>
                <input id="checkout-phone" name="phone" type="tel" autocomplete="tel" value=(&draft.phone)>
                <label for="checkout-vat">"EU company VAT number (optional, for an invoice without VAT)"</label>
                <input id="checkout-vat" name="vat_number" autocomplete="off" placeholder="DE123456789" value=(&draft.vat_number)>
                <p class="hint" data-import-notice="">"Shipping outside the EU? Import duties and taxes may be due on delivery."</p>
                <label for="checkout-note">"Note for the shop (optional)"</label>
                <textarea id="checkout-note" name="note" rows="3">(&draft.note)</textarea>
                <button class="primary">"Continue to payment"</button>
            </form>
        )
    })
}

#[derive(serde::Deserialize)]
pub struct CheckoutForm {
    // A string: `flatten` hands every field to serde as text.
    expected_total_cents: String,
    #[serde(flatten)]
    draft: CheckoutDraft,
}

/// Asks to pay for the cart. The details are kept first, so a refused form
/// comes back filled in; the payment opened for it is remembered for the
/// card form's page.
#[route(POST)]
pub async fn place(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(cart_url(cx, None)));
    };
    let draft = form.draft;
    save_checkout_draft(cx, draft.clone())?;
    let checkout = Checkout {
        email: draft.email.clone(),
        shipping: draft.shipping(),
        note: Some(draft.note.clone()),
        // Unreadable means not shown: treat it as a price that moved.
        expected_total_cents: Some(form.expected_total_cents.trim().parse().unwrap_or(u64::MAX)),
        vat_number: Some(draft.vat_number.clone()),
    };
    match start_payment(cx, &cart_id, checkout).await? {
        Ok(session) => {
            set_payment_in_progress(
                cx,
                PaymentInProgress {
                    cart_id,
                    request_id: session.request_id,
                },
            )?;
            Ok(see_other(href!(pay::index).resolve(cx)))
        }
        Err(err @ (OrdersError::EmptyCart | OrdersError::Unavailable { .. })) => {
            Ok(see_other(cart_url(cx, err.code())))
        }
        Err(err) => Ok(see_other(checkout_url(cx, err.code()))),
    }
}
