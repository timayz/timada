//! `POST /store/shoppers/sign-out` with `X-Shopper-Token`: ends that session.
//! 204, whether or not it was still open.

use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, request::headers, route},
};

use super::{Answer, SHOPPER_TOKEN, status};

#[route(POST)]
pub async fn sign_out(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    let token = headers(cx)
        .get(SHOPPER_TOKEN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    if !token.is_empty() {
        let hold = super::super::hold(cx, &access).await?;
        timada_shoppers::Command(hold.executor().clone())
            .sign_out(&token)
            .await
            .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    }
    status(StatusCode::NO_CONTENT)
}
