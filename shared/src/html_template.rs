use axum::{
    RequestPartsExt,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::{Html, IntoResponse, Response},
};
use std::convert::Infallible;

use crate::UserLanguage;

pub struct Template<T> {
    template: Option<T>,
    preferred_language: String,
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

        Ok(Template {
            template: None,
            preferred_language,
        })
    }
}

impl<T> IntoResponse for Template<T>
where
    T: askama::Template,
{
    fn into_response(self) -> Response {
        let values: (&str, &dyn std::any::Any) = ("preferred_language", &self.preferred_language);

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
