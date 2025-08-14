use axum::{extract::FromRequestParts, http::request::Parts};
use std::convert::Infallible;
use timada_shared::Metadata;
use ulid::Ulid;

impl FromRequestParts<crate::State> for Metadata {
    type Rejection = Infallible;

    async fn from_request_parts(
        _req: &mut Parts,
        _state: &crate::State,
    ) -> Result<Self, Self::Rejection> {
        Ok(Metadata {
            request_id: Ulid::new().to_string(),
            request_by: "".to_owned(),
            request_as: None,
        })
    }
}
