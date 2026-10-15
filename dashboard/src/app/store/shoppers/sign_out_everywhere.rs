//! `POST /store/shoppers/sign-out-everywhere` with `X-Shopper-Token`: ends
//! every other session of the shopper; the one the token opened stays. 204.

use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, route},
};

use super::{Answer, SIGNED_OUT, answer_error, refused, shopper_token, signed_in, status};

#[route(POST)]
pub async fn sign_out_everywhere(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let hold = super::super::hold(cx, &access).await?;
    let (Some(shopper), Some(token)) = (signed_in(cx, hold.executor()).await?, shopper_token(cx))
    else {
        return refused(StatusCode::UNAUTHORIZED, SIGNED_OUT);
    };
    match timada_shoppers::Command(hold.executor().clone())
        .revoke_other_sessions(&shopper.id, &token)
        .await
    {
        Ok(()) => status(StatusCode::NO_CONTENT),
        Err(err) => answer_error(err),
    }
}
