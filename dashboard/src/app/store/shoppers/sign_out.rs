//! `POST /store/shoppers/sign_out` with `X-Shopper-Token`: ends that session.
//! 204, whether or not it was still open.

use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, route},
};

use super::{Answer, shopper_token, status};

#[route(POST)]
pub async fn sign_out(cx: &Cx) -> Result<Answer> {
    let access = match super::super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return status(code),
    };
    if let Some(token) = shopper_token(cx) {
        let store = super::super::store(cx, &access);
        timada_shoppers::Command::new(store.executor().clone(), store.tenant_id())
            .sign_out(&token)
            .await
            .map_err(|err| topcoat::Error::from_anyhow(err.into()))?;
    }
    status(StatusCode::NO_CONTENT)
}
