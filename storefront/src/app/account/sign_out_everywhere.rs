//! `POST /account/sign-out-everywhere`: signs the shopper out of every other
//! browser, then back to the account. This one stays signed in.

use timada_storefront::sign_out_everywhere_else;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use crate::app::account::{account_done_url, account_url};
use crate::shopper::{forget_shopper, shopper_token};

/// What the account page says once done.
pub const DONE: &str = "signed_out_elsewhere";

#[route(POST)]
pub async fn sign_out_everywhere(cx: &Cx) -> Result<SeeOther> {
    let Some(token) = shopper_token(cx) else {
        return Ok(see_other(account_url(cx, None)));
    };
    match sign_out_everywhere_else(cx, &token).await? {
        Ok(()) => Ok(see_other(account_done_url(cx, DONE))),
        Err(refusal) => {
            if refusal.is_signed_out() {
                forget_shopper(cx);
            }
            Ok(see_other(account_url(cx, Some(refusal.code()))))
        }
    }
}
