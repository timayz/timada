//! When the dashboard cannot be reached.
//!
//! The catalog is read from the local copy, so a dashboard that is down
//! leaves every page that only reads it untouched. What goes to the
//! dashboard (the cart, the checkout, a sign-in, a review, a message, a
//! newsletter sign-up) cannot, and the failure is one the shopper can be
//! told about: nothing is wrong with what they asked, the feature is away
//! for a moment. So every call of the library that reaches the dashboard
//! fails with [`StoreUnavailable`] in that case, rather than with the
//! transport's own error, and the host tells the two apart with
//! [`is_store_unavailable`] (in a layer, say, that answers such a failure
//! with a page of its own and a 503 instead of the router's 500).
//!
//! What counts as unreachable: the connection to the dashboard's HTTP side
//! failing or timing out; a proxy in front of it answering 502, 503 or 504
//! for it (a load balancer's "no available server" page, not a refusal the
//! dashboard wrote); the store API connection not being up, or a request
//! on it failing. A refusal the dashboard answered (a 422, or a 503 whose
//! body carries `writes_paused`) is not this: it is a refusal to show.

use std::fmt;

use topcoat_core::error::Error;

/// The refusal code a page may carry for [`StoreUnavailable`], beside the
/// codes the dashboard answers with.
pub const STORE_UNAVAILABLE: &str = "store_unavailable";

/// The dashboard (or the store API) cannot be reached for the moment: the
/// feature is away, not refused. Carried as the error of every library
/// call that needed it; see [the module](self).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoreUnavailable {
    /// What failed, for the log.
    cause: String,
}

impl StoreUnavailable {
    pub fn new(cause: impl fmt::Display) -> Self {
        Self {
            cause: cause.to_string(),
        }
    }

    /// What failed, as the transport said it.
    pub fn cause(&self) -> &str {
        &self.cause
    }

    /// The refusal code a page carries for it ([`STORE_UNAVAILABLE`]).
    pub fn code(&self) -> &'static str {
        STORE_UNAVAILABLE
    }
}

impl fmt::Display for StoreUnavailable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the dashboard cannot be reached for the moment: {}",
            self.cause
        )
    }
}

impl std::error::Error for StoreUnavailable {}

/// Whether a page's error is the dashboard being away
/// ([`StoreUnavailable`]), for a host to answer it with a page of its own.
pub fn is_store_unavailable(err: &Error) -> bool {
    err.is::<StoreUnavailable>()
}

/// What a library call's failure becomes at a page: [`StoreUnavailable`]
/// kept as itself, so [`is_store_unavailable`] finds it; anything else
/// crosses as an opaque error (a 500).
pub(crate) fn into_error(err: anyhow::Error) -> Error {
    match err.downcast::<StoreUnavailable>() {
        Ok(unavailable) => Error::from(unavailable),
        Err(err) => Error::from_anyhow(err),
    }
}

/// Whether an `anyhow` failure of a [`Store`](crate::Store) method is the
/// dashboard being away, for code that runs outside a request.
pub fn is_unavailable(err: &anyhow::Error) -> bool {
    err.downcast_ref::<StoreUnavailable>().is_some()
}

/// The dashboard's answer, or [`StoreUnavailable`] when the request never
/// got one: the connection failed, timed out, or broke while answering.
pub(crate) fn reach(
    response: reqwest::Result<reqwest::Response>,
) -> anyhow::Result<reqwest::Response> {
    response.map_err(|err| StoreUnavailable::new(format!("{err:#}")).into())
}

/// The body of the dashboard's answer, or [`StoreUnavailable`] when the
/// connection broke while it was being read.
pub(crate) async fn reach_body(response: reqwest::Response) -> anyhow::Result<bytes::Bytes> {
    response
        .bytes()
        .await
        .map_err(|err| StoreUnavailable::new(format!("{err:#}")).into())
}

/// Whether `status` is a proxy answering for a dashboard that is not
/// there (502, 503, 504) with something other than a refusal the dashboard
/// wrote: a 503 whose body is JSON with an `error` is the dashboard's own
/// (`writes_paused`), anything else at those statuses is the proxy's.
pub(crate) fn gateway_down(status: reqwest::StatusCode, body: &[u8]) -> Option<StoreUnavailable> {
    if !matches!(status.as_u16(), 502..=504) {
        return None;
    }
    let refusal = serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .is_some_and(|value| value.get("error").is_some());
    (!refusal).then(|| StoreUnavailable::new(format!("the dashboard answered {status}")))
}

/// A read on the store API that failed: the connection is down or the
/// request timed out, which to a shopper is the same thing.
pub(crate) fn store_away(err: anyhow::Error) -> anyhow::Error {
    if is_unavailable(&err) {
        return err;
    }
    StoreUnavailable::new(format!("{err:#}")).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proxy_answer_at_a_gateway_status_is_the_dashboard_away() {
        let html = b"<html>503 Service Unavailable: no available server</html>";
        assert!(gateway_down(reqwest::StatusCode::SERVICE_UNAVAILABLE, html).is_some());
        assert!(gateway_down(reqwest::StatusCode::BAD_GATEWAY, b"").is_some());
        assert!(gateway_down(reqwest::StatusCode::GATEWAY_TIMEOUT, b"").is_some());
    }

    #[test]
    fn a_refusal_the_dashboard_wrote_is_not() {
        let paused = br#"{"error":"writes_paused"}"#;
        assert!(gateway_down(reqwest::StatusCode::SERVICE_UNAVAILABLE, paused).is_none());
        assert!(gateway_down(reqwest::StatusCode::UNPROCESSABLE_ENTITY, b"").is_none());
        assert!(gateway_down(reqwest::StatusCode::INTERNAL_SERVER_ERROR, b"").is_none());
    }

    #[test]
    fn the_failure_keeps_its_type_across_the_page_boundary() {
        let err: anyhow::Error = StoreUnavailable::new("connection refused").into();
        assert!(is_unavailable(&err));
        let err = err.context("cannot read the cart");
        assert!(is_unavailable(&err), "anyhow context keeps it reachable");
        assert!(is_store_unavailable(&into_error(err)));
        assert!(!is_store_unavailable(&into_error(anyhow::anyhow!(
            "database is locked"
        ))));
    }

    #[test]
    fn any_failure_of_a_store_read_is_the_store_away() {
        let err = store_away(anyhow::anyhow!("remote request timed out after 10s"));
        assert!(is_unavailable(&err));
        assert!(format!("{err}").contains("timed out"));
    }
}
