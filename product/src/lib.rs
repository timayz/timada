mod error;
pub mod product;
mod routes;

use std::ops::Deref;

use axum::{
    Extension,
    extract::{FromRequestParts, rejection::ExtensionRejection},
    http::request::Parts,
    middleware::AddExtension,
};
pub use routes::create_router;

rust_i18n::i18n!("locales");

pub(crate) mod filters {
    #[allow(dead_code)]
    pub fn t(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let preferred_language = askama::get_value::<String>(values, "preferred_language")
            .expect("Unable to get preferred_language from askama::get_value");

        Ok(rust_i18n::t!(value, locale = preferred_language).to_string())
    }

    #[allow(dead_code)]
    pub fn assets(value: &str, values: &dyn askama::Values) -> askama::Result<String> {
        let config = askama::get_value::<shared::TemplateConfig>(values, "config")
            .expect("Unable to get config from askama::get_value");

        Ok(format!("{}/{value}", config.assets_base_url))
    }
}

#[derive(Clone)]
pub struct State {
    pub executor: liteventd::sql::Sql<sqlx::Sqlite>,
    pub region: String,
}

impl<S> tower_layer::Layer<S> for State {
    type Service = AddExtension<S, Self>;

    fn layer(&self, inner: S) -> Self::Service {
        Extension(self.clone()).layer(inner)
    }
}

impl<S> FromRequestParts<S> for State
where
    S: Send + Sync,
{
    type Rejection = ExtensionRejection;

    async fn from_request_parts(req: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = Extension::<State>::from_request_parts(req, state).await?;

        Ok(state.deref().clone())
    }
}
