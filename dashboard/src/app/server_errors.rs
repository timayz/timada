//! Logs what a request failed with when the answer is a bare 500.
//!
//! The router turns an error it does not know into "internal server error"
//! and drops it, so without this a failed write leaves nothing behind to say
//! why. The errors it does know (not found, forbidden, a redirect, …) are
//! answers, not failures, and pass through unlogged.

use topcoat::{
    context::Cx,
    router::{
        Body, Layer, LayerFuture, Next, Path,
        error::{
            BadRequestError, ContentTooLargeError, ForbiddenError, MethodNotAllowedError,
            NotFoundError, RedirectError, SeeOther, ServiceUnavailableError, TooManyRequestsError,
            UnauthorizedError,
        },
        request::{method, uri},
    },
};

/// Wraps every request; register it on the router with no path.
pub struct LogServerErrors;

impl Layer for LogServerErrors {
    fn path(&self) -> Option<&Path> {
        None
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let result = next.run(cx, body).await;
            if let Err(error) = &result
                && !is_an_answer(error)
            {
                tracing::error!(
                    method = %method(cx),
                    path = uri(cx).path(),
                    error = format!("{error:#}"),
                    "request failed with an internal server error"
                );
            }
            result
        })
    }
}

/// Whether the router renders `error` as a response of its own rather than
/// a 500.
fn is_an_answer(error: &topcoat::Error) -> bool {
    error.is::<NotFoundError>()
        || error.is::<ForbiddenError>()
        || error.is::<UnauthorizedError>()
        || error.is::<BadRequestError>()
        || error.is::<ContentTooLargeError>()
        || error.is::<MethodNotAllowedError>()
        || error.is::<RedirectError>()
        || error.is::<SeeOther>()
        || error.is::<ServiceUnavailableError>()
        || error.is::<TooManyRequestsError>()
}

#[cfg(test)]
mod tests {
    use topcoat::router::error::{forbidden, not_found};

    use super::is_an_answer;

    #[test]
    fn only_what_becomes_a_bare_500_is_a_failure() {
        assert!(is_an_answer(&not_found().into()));
        assert!(is_an_answer(&forbidden().into()));
        assert!(!is_an_answer(&topcoat::Error::from_anyhow(
            anyhow::anyhow!("database is locked")
        )));
    }
}
