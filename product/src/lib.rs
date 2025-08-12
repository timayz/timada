mod routes;

use axum::{Extension, middleware::AddExtension};
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
}

impl<S> tower_layer::Layer<S> for State {
    type Service = AddExtension<S, Self>;

    fn layer(&self, inner: S) -> Self::Service {
        Extension(self.clone()).layer(inner)
    }
}
