use axum::{extract::FromRequestParts, http::request::Parts};
use std::convert::Infallible;
use ulid::Ulid;

impl FromRequestParts<crate::State> for shared::Metadata {
    type Rejection = Infallible;

    async fn from_request_parts(
        _req: &mut Parts,
        _state: &crate::State,
    ) -> Result<Self, Self::Rejection> {
        Ok(shared::Metadata {
            request_id: Ulid::new().to_string(),
            request_by: "".to_owned(),
            request_as: None,
        })
    }
}
