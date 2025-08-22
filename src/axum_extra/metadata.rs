use axum::{extract::FromRequestParts, http::request::Parts};
use std::convert::Infallible;
use timada_shared::RequestMetadata;
use ulid::Ulid;

impl FromRequestParts<crate::State> for RequestMetadata {
    type Rejection = Infallible;

    async fn from_request_parts(
        _req: &mut Parts,
        _state: &crate::State,
    ) -> Result<Self, Self::Rejection> {
        Ok(RequestMetadata {
            id: Ulid::new().to_string(),
            user_id: "".to_owned(),
            user_owner_id: None,
        })
    }
}
