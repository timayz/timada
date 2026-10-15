//! `POST /store/shoppers/delete` with `X-Shopper-Token`: deletes the
//! shopper's account. The addresses are forgotten and every session ends,
//! this one included; the orders placed with the address stay with the shop.
//! 204.

use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, route},
};

use super::{Answer, SIGNED_OUT, answer_error, refused, signed_in, status};

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = super::super::hold(cx, &access).await?;
    let Some(shopper) = signed_in(cx, hold.executor()).await? else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    match timada_shoppers::Command(hold.executor().clone())
        .forget(&shopper.id)
        .await
    {
        Ok(()) => status(StatusCode::NO_CONTENT),
        Err(err) => answer_error(err),
    }
}
