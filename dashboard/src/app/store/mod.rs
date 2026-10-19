//! What a storefront calls over HTTP, with its store API key: the writes the
//! store API does not carry, because they come with files (reviews), must
//! not be forged by whoever holds a key (shopper sign-ins), or come from
//! anyone at all and are counted (the contact form's messages).

pub mod contact;
pub mod newsletter;
pub mod reviews;
pub mod shoppers;

use std::sync::Arc;

use timada_tenancy::{ApiKeyAccess, SCOPE_STORE_CHECKOUT, TenancyError};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{StatusCode, header::AUTHORIZATION, request::headers},
};

use crate::app::tenants::tenancy;
use crate::tenants::{RequestHold, Tenants};

/// The storefront behind a request: its key (`Authorization: Bearer tk_…`)
/// names the tenant and must allow `store:checkout`. `Err` is the status to
/// answer: 401 for a missing, unknown or revoked key, 403 for a key that may
/// not sell, 503 for a suspended tenant.
pub async fn storefront(cx: &Cx) -> Result<std::result::Result<ApiKeyAccess, StatusCode>> {
    let token = headers(cx)
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let Some(token) = token else {
        return Ok(Err(StatusCode::UNAUTHORIZED));
    };
    let access = match tenancy(cx).authenticate_api_key(token).await {
        Ok(access) => access,
        Err(TenancyError::ApiKeyInvalid) => return Ok(Err(StatusCode::UNAUTHORIZED)),
        Err(TenancyError::Suspended) => return Ok(Err(StatusCode::SERVICE_UNAVAILABLE)),
        Err(err) => return Err(topcoat::Error::from_anyhow(err.into())),
    };
    if !access.key.allows(SCOPE_STORE_CHECKOUT) {
        return Ok(Err(StatusCode::FORBIDDEN));
    }
    Ok(Ok(access))
}

/// The tenant's store, open for the length of the request.
pub async fn hold(cx: &Cx, access: &ApiKeyAccess) -> Result<RequestHold> {
    app_context::<Arc<Tenants>>(cx)
        .hold(&access.tenant)
        .await
        .map_err(topcoat::Error::from_anyhow)
}
