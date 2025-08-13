mod html_template;
mod user_lang;

use std::convert::Infallible;

use axum::{extract::FromRequestParts, http::request::Parts};
pub use html_template::{Template, TemplateConfig};
use serde::{Deserialize, Serialize};
use ulid::Ulid;
pub use user_lang::*;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub request_id: String,
    pub request_by: String,
    pub request_as: Option<String>,
}

impl<S> FromRequestParts<S> for Metadata
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(_req: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Metadata {
            request_id: Ulid::new().to_string(),
            request_by: "".to_owned(),
            request_as: None,
        })
    }
}
