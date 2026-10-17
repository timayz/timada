//! `POST /checkout/start`: the page's script asks to pay here, then
//! confirms the payment with Stripe.

use topcoat::{
    Result,
    context::Cx,
    router::{
        content::{Form, Json},
        route,
    },
};

use super::{Asked, CheckoutForm, ask};
use crate::app::say;

/// What the page's script reads back.
#[derive(Debug, Default, serde::Serialize)]
pub struct Started {
    #[serde(skip_serializing_if = "Option::is_none")]
    client_secret: Option<String>,
    /// Why not, as the shopper reads it.
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
    /// Where to go instead: the cart, or this page priced afresh.
    #[serde(skip_serializing_if = "Option::is_none")]
    redirect: Option<String>,
}

/// Asks to pay for the page's script, which confirms with Stripe next.
#[route(POST)]
pub async fn start(cx: &Cx, Form(form): Form<CheckoutForm>) -> Result<Json<Started>> {
    let asked = ask(cx, form).await?;
    let started = match &asked {
        Asked::Open(client_secret) => Started {
            client_secret: Some(client_secret.clone()),
            ..Started::default()
        },
        // The page's total, and the amount Stripe's form holds, are stale
        // (a price, or the shipping methods, changed); so is the cart on a
        // refusal it fixes: load the page again.
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
            error: code.map(say),
            ..Started::default()
        },
    };
    Ok(Json(started))
}
