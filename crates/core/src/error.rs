use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Framework-wide handler error.
///
/// Converts to an HTTP response; anything convertible to `anyhow::Error`
/// converts into [`AppError::Internal`], so handlers can use `?` freely.
#[derive(Debug)]
pub enum AppError {
    NotFound,
    BadRequest(String),
    Internal(anyhow::Error),
}

pub type AppResult<T> = Result<T, AppError>;

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "not found").into_response(),
            AppError::BadRequest(reason) => (StatusCode::BAD_REQUEST, reason).into_response(),
            AppError::Internal(source) => {
                tracing::error!(error = ?source, "internal server error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
            }
        }
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(source: E) -> Self {
        AppError::Internal(source.into())
    }
}
