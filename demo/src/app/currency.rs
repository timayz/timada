//! `POST /currency`: the shopper picks the currency the shop is shown — and
//! charged — in. With articles in the cart, which are priced in another
//! currency, the switch is first confirmed on the cart page: it empties the
//! cart.

use serde::Deserialize;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::bad_request, error::see_other, href, page},
    view::View,
};

use super::{cart, safe_next};
use crate::{
    Store,
    cart_session::current_cart,
    currency::{cart_currency, choose_currency},
    db::shop_currencies,
};

#[derive(Debug, Deserialize)]
pub struct SwitchForm {
    currency: String,
    /// Where to go back to: a path of this site.
    next: Option<String>,
    /// Set by the cart page's confirmation: the shopper agreed to lose the
    /// cart's articles.
    empty_cart: Option<String>,
}

#[page(POST "/currency")]
pub async fn switch(cx: &Cx, Form(form): Form<SwitchForm>) -> Result<impl View> {
    if !shop_currencies().sells_in(&form.currency) {
        return Err(bad_request(format!("the shop does not sell in {}", form.currency)).into());
    }
    let next = safe_next(form.next).unwrap_or_else(|| "/".to_owned());

    // A cart in hand was priced in its own currency, and never mixes two.
    if cart_currency(cx)
        .await?
        .is_some_and(|held| held != form.currency)
    {
        if form.empty_cart.is_none() {
            let confirm = href!(cart::show)
                .query([("devise", form.currency.as_str()), ("next", next.as_str())])
                .resolve(cx);
            return Err(see_other(confirm).into());
        }
        let store = app_context::<Store>(cx);
        let cart = match current_cart(cx).await {
            Ok(cart) => cart.clone(),
            Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
        };
        if let Some(cart) = cart {
            let carts = timada_cart::Command(&store.executor);
            for line in &cart.lines {
                carts
                    .remove_line(&cart.id, line.product_id.clone())
                    .await
                    .map_err(anyhow::Error::from)?;
            }
        }
    }
    choose_currency(cx, &form.currency);
    Err::<(), _>(see_other(next).into())
}
