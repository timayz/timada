//! `POST /account/sign-out`: ends the shopper's session, then home.

use timada_storefront::sign_out as end_session;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, route,
    },
};

use crate::shopper::{forget_shopper, shopper_token};

#[route(POST)]
pub async fn sign_out(cx: &Cx) -> Result<SeeOther> {
    if let Some(token) = shopper_token(cx) {
        end_session(cx, &token).await?;
    }
    forget_shopper(cx);
    Ok(see_other(href!(crate::app::index).resolve(cx)))
}
