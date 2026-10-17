//! Paying: the checkout form asks to pay (`start`, for the page's script,
//! which confirms with the provider next; `place`, the form sent without
//! it), and the legacy card form page redirects to the checkout.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::content::{Form, Json};
use topcoat_router::error::{RedirectError, SeeOther, redirect, see_other};

use crate::cart::{Checkout, OrdersError, accepts_business_buyers, start_payment};
use crate::session::{
    CheckoutDraft, PaymentInProgress, accounts_enabled, cart_id, save_checkout_draft,
    set_payment_in_progress, shopper_token, signed_in,
};
use crate::shoppers::{request_language, save_address};
use crate::storefront::{messages, paths};

use super::{action, query};

/// What the checkout form posts.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct CheckoutForm {
    // A string: `flatten` hands every field to serde as text.
    pub expected_total_cents: String,
    /// Signed in: keep the address on the account.
    #[serde(default)]
    pub save_address: String,
    /// Signed in: keep the billing address on the account too.
    #[serde(default)]
    pub save_billing_address: String,
    #[serde(flatten)]
    pub draft: CheckoutDraft,
}

/// How a request to pay ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    /// A payment is open; the browser confirms it with this secret.
    Open(String),
    /// Refused, with the reason; the cart is where some are fixed.
    Refused {
        code: Option<&'static str>,
        to_cart: bool,
    },
}

impl Asked {
    /// The page a refusal sends the shopper back to.
    pub fn back(&self, cx: &Cx) -> String {
        let paths = paths(cx);
        match self {
            Asked::Refused {
                code,
                to_cart: true,
            } => paths.cart_url(*code),
            Asked::Refused { code, .. } => paths.checkout_url(*code),
            Asked::Open(_) => paths.checkout_url(None),
        }
    }
}

/// Asks to pay for the cart. The details are kept first, so a refused form
/// comes back filled in; the payment opened for it is remembered for the
/// return page.
pub async fn ask(cx: &Cx, form: CheckoutForm) -> Result<Asked> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(Asked::Refused {
            code: None,
            to_cart: true,
        });
    };
    let mut draft = form.draft;
    // A signed-in shopper orders under their account's address, whatever
    // the form said.
    let token = shopper_token(cx).filter(|_| accounts_enabled(cx));
    if token.is_some()
        && let Some(shopper) = signed_in(cx).await
    {
        draft.email = shopper.email.clone();
    }
    save_checkout_draft(cx, draft.clone())?;
    let business = accepts_business_buyers(cx).await?;
    let checkout = Checkout {
        email: draft.email.clone(),
        shipping: draft.shipping(),
        note: Some(draft.note.clone()),
        // Unreadable means not shown: treat it as a price that moved.
        expected_total_cents: Some(form.expected_total_cents.trim().parse().unwrap_or(u64::MAX)),
        // A number kept from before the shop stopped selling to businesses
        // is not sent: the field is not shown any more.
        vat_number: business.then(|| draft.vat_number.clone()),
        billing: draft.billing(),
        shipping_method: Some(draft.shipping_method.clone()),
        // The shop mails about the order in the shopper's language.
        language: request_language(cx),
    };
    let err = match start_payment(cx, &cart_id, checkout).await? {
        Ok(session) => {
            // The addresses checked out: kept on the account for the next
            // order. A refusal (ten kept already) does not stop the payment.
            // The billing one first: the latest saved is the next delivery.
            if let Some(token) = &token {
                if let Some(billing) = draft
                    .billing()
                    .filter(|_| !form.save_billing_address.is_empty())
                {
                    let _ = save_address(cx, token, &billing).await?;
                }
                if !form.save_address.is_empty() {
                    let _ = save_address(cx, token, &draft.shipping()).await?;
                }
            }
            set_payment_in_progress(
                cx,
                PaymentInProgress {
                    cart_id,
                    request_id: session.request_id,
                },
            )?;
            return Ok(Asked::Open(session.client_secret));
        }
        Err(err) => err,
    };
    let code = err.code();
    let to_cart = matches!(err, OrdersError::EmptyCart | OrdersError::Unavailable { .. })
        // The cart is where the code is changed or taken off.
        || code.is_some_and(|code| code.starts_with("discount_"));
    Ok(Asked::Refused { code, to_cart })
}

/// What the page's script reads back from `start`.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Started {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    /// Why not, as the shopper reads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Where to go instead: the cart, or the checkout priced afresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect: Option<String>,
}

/// Asks to pay for the page's script, which confirms with the provider next.
pub(crate) async fn start(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<Json<Started>> {
    let asked = ask(cx, form).await?;
    let started = match &asked {
        Asked::Open(client_secret) => Started {
            client_secret: Some(client_secret.clone()),
            ..Started::default()
        },
        // The page's total, and the amount the provider's form holds, are
        // stale (a price, or the shipping methods, changed); so is the cart
        // on a refusal it fixes: load the page again.
        Asked::Refused {
            code: Some("price_changed" | "shipping_unavailable"),
            ..
        }
        | Asked::Refused { to_cart: true, .. }
        | Asked::Refused { code: None, .. } => Started {
            redirect: Some(asked.back(cx)),
            ..Started::default()
        },
        Asked::Refused { code, .. } => Started {
            error: code.map(|code| messages(cx).say(code).to_owned()),
            ..Started::default()
        },
    };
    Ok(Json(started))
}

/// The form sent without the page's script: the provider's form needs it,
/// so a payment cannot be confirmed this way; a refusal is still worded.
pub(crate) async fn place(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<SeeOther> {
    let asked = ask(cx, form).await?;
    Ok(see_other(asked.back(cx)))
}

#[derive(serde::Deserialize)]
struct PayQuery {
    error: Option<String>,
}

/// Where the card form used to be: a bookmark or a stale return lands on
/// the checkout.
pub(crate) async fn pay(cx: &Cx) -> Result<RedirectError> {
    let query: PayQuery = query(cx)?;
    Ok(redirect(paths(cx).checkout_url(query.error.as_deref())))
}

action!(start_route = start, Form<CheckoutForm>);
action!(place_route = place, Form<CheckoutForm>);
action!(pay_route = pay);
