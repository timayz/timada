//! `/store/shoppers/*`: a storefront signs its shoppers in through here. The
//! dashboard issues and checks the mailed codes and opens the sessions, so a
//! store API key cannot sign anyone in by writing events; the storefront only
//! keeps the session token in its shopper's browser.
//!
//! Every call carries the storefront's key (see [`super::storefront`]); the
//! calls made for a signed-in shopper also carry their token in
//! `X-Shopper-Token`. Bodies are form-encoded, answers JSON: a refusal to show
//! the shopper is 422 `{"error": code}` (the codes of
//! `timada_shoppers::SHOPPER_CODES`), a token that no longer signs anyone in
//! is 401 `{"error": "signed_out"}`.

pub mod addresses;
pub mod code;
pub mod delete;
pub mod me;
pub mod newsletter;
pub mod orders;
pub mod session;
pub mod sign_out;
pub mod sign_out_everywhere;

use std::sync::Arc;

use serde_json::{Value, json};
use timada_core::{CodeVerifier, Keys};
use timada_shoppers::{Shopper, ShopperError, ShopperState};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, content::Json, request::headers},
};

use crate::auth::now;
use crate::key_vault::KeyVault;

/// The header a signed-in shopper's token travels in.
pub const SHOPPER_TOKEN: &str = "x-shopper-token";

/// What a token answered for no longer signs anyone in.
pub const SIGNED_OUT: &str = "signed_out";

/// This node holds no `secrets.key`, so no shopper can sign in through it.
pub const SHOPPER_ACCOUNTS_UNAVAILABLE: &str = "shopper_accounts_unavailable";

pub type Answer = (StatusCode, Json<Value>);

pub fn answer(status: StatusCode, body: Value) -> Result<Answer> {
    Ok((status, Json(body)))
}

/// A bare status, for a key that does not open the store.
pub fn status(status: StatusCode) -> Result<Answer> {
    answer(status, json!({}))
}

pub fn refused(status: StatusCode, code: &str) -> Result<Answer> {
    answer(status, json!({ "error": code }))
}

/// 422 with the refusal's code, or the error itself for an infrastructure
/// failure.
pub fn answer_error(err: ShopperError) -> Result<Answer> {
    match err.code() {
        // The shop is not taking writes for the moment: not the shopper's
        // doing, so a 503 the storefront can word as "try again later".
        Some(code) if code == timada_core::WRITES_PAUSED => {
            refused(StatusCode::SERVICE_UNAVAILABLE, code)
        }
        Some(code) if err.is_user_facing() => {
            let status = if matches!(err, ShopperError::CodeRecentlySent) {
                StatusCode::TOO_MANY_REQUESTS
            } else {
                StatusCode::UNPROCESSABLE_ENTITY
            };
            refused(status, code)
        }
        _ => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

/// The shopper token the request carries, if any.
pub fn shopper_token(cx: &Cx) -> Option<String> {
    headers(cx)
        .get(SHOPPER_TOKEN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
}

/// The shopper whose token the request carries, if it still signs them in.
pub async fn signed_in(cx: &Cx, store: &super::TenantStore) -> Result<Option<ShopperState>> {
    let Some(token) = shopper_token(cx) else {
        return Ok(None);
    };
    let resolved = timada_shoppers::Command::new(store.executor().clone(), store.tenant_id())
        .resolve_session(&token, now())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(resolved.map(|resolved| resolved.shopper))
}

/// The keys of the tenant's personal data (its shoppers', its messages').
pub fn keys(cx: &Cx, tenant_id: &str) -> Arc<dyn Keys> {
    app_context::<Arc<KeyVault>>(cx).for_tenant(tenant_id)
}

/// What checks the shoppers' sign-in codes; `Err` is the answer to give when
/// this node holds no `secrets.key`.
pub fn codes(cx: &Cx) -> std::result::Result<Arc<dyn CodeVerifier>, Result<Answer>> {
    match &app_context::<crate::seal::ShopperCodes>(cx).0 {
        Some(codes) => Ok(codes.clone()),
        None => Err(refused(
            StatusCode::SERVICE_UNAVAILABLE,
            SHOPPER_ACCOUNTS_UNAVAILABLE,
        )),
    }
}

/// The shopper's personal data, opened with their key: `None` once the key
/// is destroyed (the account was deleted meanwhile).
pub async fn opened(cx: &Cx, tenant_id: &str, shopper: &ShopperState) -> Result<Option<Shopper>> {
    shopper
        .open_with(&*keys(cx, tenant_id))
        .await
        .map_err(topcoat::Error::from_anyhow)
}
