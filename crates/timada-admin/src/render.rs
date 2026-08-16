use askama::Template;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

/// Error type for every admin handler.
#[derive(Debug, thiserror::Error)]
pub enum AdminError {
    #[error("not found")]
    NotFound,
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AdminError {
    fn into_response(self) -> Response {
        match self {
            AdminError::NotFound => (StatusCode::NOT_FOUND, "Not found").into_response(),
            AdminError::Internal(error) => {
                tracing::error!(error = ?error, "admin request failed");
                (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
            }
        }
    }
}

/// Render an Askama template as a `text/html` response.
pub(crate) fn html<T: Template>(template: &T) -> Result<Response, AdminError> {
    let body = template.render().map_err(anyhow::Error::from)?;
    Ok((
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response())
}
