//! `POST /account/newsletter` (`subscribed=true|false`): the signed-in
//! shopper puts themselves on the shop's newsletter, or takes themselves off
//! it, then back to the account. Their sign-in proved their address, so no
//! confirmation link is mailed.

use std::collections::HashMap;

use timada_storefront::set_shopper_newsletter;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::account::{account_done_url, account_url};
use crate::shopper::{forget_shopper, shopper_token};

/// What the account page says once subscribed.
pub const SUBSCRIBED: &str = "newsletter_on";
/// What the account page says once unsubscribed.
pub const UNSUBSCRIBED: &str = "newsletter_off";

#[route(POST)]
pub async fn newsletter(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(account_url(cx, None)));
    };
    let subscribed = form.get("subscribed").is_some_and(|value| value == "true");
    match set_shopper_newsletter(cx, &token, subscribed).await? {
        Ok(()) => Ok(see_other(account_done_url(
            cx,
            if subscribed { SUBSCRIBED } else { UNSUBSCRIBED },
        ))),
        Err(refusal) => {
            if refusal.is_signed_out() {
                forget_shopper(cx);
            }
            Ok(see_other(account_url(cx, Some(refusal.code()))))
        }
    }
}
