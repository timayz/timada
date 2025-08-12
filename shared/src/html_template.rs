use axum::{
    Extension, RequestPartsExt,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    middleware::AddExtension,
    response::{Html, IntoResponse, Response},
};
use std::{collections::HashMap, convert::Infallible};

use crate::UserLanguage;

#[derive(Clone)]
pub struct TemplateConfig {
    pub assets_base_url: String,
}

impl TemplateConfig {
    pub fn new(assets_base_url: impl Into<String>) -> Self {
        Self {
            assets_base_url: assets_base_url.into(),
        }
    }
}

impl<S> tower_layer::Layer<S> for TemplateConfig {
    type Service = AddExtension<S, Self>;

    fn layer(&self, inner: S) -> Self::Service {
        Extension(self.clone()).layer(inner)
    }
}

pub struct Template<T> {
    template: Option<T>,
    preferred_language: String,
    preferred_language_iso: String,
    config: TemplateConfig,
}

impl<T> Template<T> {
    pub fn template(mut self, t: T) -> Self {
        self.template = Some(t);

        self
    }
}

impl<S, T> FromRequestParts<S> for Template<T>
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let user_language = parts
            .extract::<UserLanguage>()
            .await
            .expect("Unable to extract user languages");

        let preferred_language = user_language
            .preferred_languages()
            .first()
            .cloned()
            .unwrap_or_else(|| "en".to_owned());

        let preferred_language_iso = preferred_language
            .split_once("-")
            .unwrap_or((preferred_language.as_str(), ""))
            .0
            .to_owned();

        let config = parts
            .extensions
            .get::<TemplateConfig>()
            .expect("TemplateConfig not configured")
            .to_owned();

        Ok(Template {
            template: None,
            preferred_language,
            preferred_language_iso,
            config,
        })
    }
}

impl<T> IntoResponse for Template<T>
where
    T: askama::Template,
{
    fn into_response(self) -> Response {
        let mut values: HashMap<&str, Box<dyn std::any::Any>> = HashMap::new();
        values.insert("preferred_language", Box::new(self.preferred_language));
        values.insert(
            "preferred_language_iso",
            Box::new(self.preferred_language_iso),
        );
        values.insert("config", Box::new(self.config));

        match self
            .template
            .expect("template must be define using template.template(..)")
            .render_with_values(&values)
        {
            Ok(html) => Html(html).into_response(),
            Err(err) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to render template. Error: {err}"),
            )
                .into_response(),
        }
    }
}
