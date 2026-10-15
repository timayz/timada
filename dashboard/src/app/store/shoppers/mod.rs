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
pub mod orders;
pub mod session;
pub mod sign_out;
pub mod sign_out_everywhere;

use serde_json::{Value, json};
use timada_shoppers::{ShopperError, ShopperState};
use topcoat::{
    Result,
    context::Cx,
    router::{StatusCode, content::Json, request::headers},
};

use crate::auth::now;
use crate::cluster::Executor;

/// The header a signed-in shopper's token travels in.
pub const SHOPPER_TOKEN: &str = "x-shopper-token";

/// What a token answered for no longer signs anyone in.
pub const SIGNED_OUT: &str = "signed_out";

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
pub async fn signed_in(cx: &Cx, executor: &Executor) -> Result<Option<ShopperState>> {
    let Some(token) = shopper_token(cx) else {
        return Ok(None);
    };
    let resolved = timada_shoppers::Command(executor.clone())
        .resolve_session(&token, now())
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    Ok(resolved.map(|resolved| resolved.shopper))
}
