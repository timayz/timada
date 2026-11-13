//! How a page reads what every request asks first (#466).
//!
//! Every page reads the signed-in browser's session and its user, and every
//! tenant page the tenant, before anything else. Through the store, on a
//! cluster, each of those reads waits for a consensus round with a quorum. So
//! they are read from this node's file ([`LocalReads::first`]), and asked of
//! the cluster only when the answer would turn the request away.
//!
//! That leaves what the browser changed itself. The page a form sends it back
//! to must show the change whichever node answers, and the node may be a
//! moment behind the one that took the write. So the answer to a request that
//! may have written (a form's post, or a redirect, which is how the sign-in
//! and connection callbacks end after writing) sets [`WROTE_COOKIE`] for
//! [`WROTE_FOR`]. While the browser sends it back, the request reads through
//! the cluster.

use std::future::Future;

use topcoat::{
    context::{Cx, app_context},
    cookie::{Cookie, Cookies, SameSite, cookies, time},
    router::{
        Body, Layer, LayerFuture, Method, Next, Path,
        error::{RedirectError, SeeOther},
        request::method,
    },
};

use crate::cluster::{Executor, LocalReads};

/// The cookie that says the browser may have written a moment ago.
pub const WROTE_COOKIE: &str = "timada_wrote";

/// How long after a write the browser's requests read through the cluster:
/// far longer than a node the balancer sends traffic to takes to apply what
/// its peers committed.
const WROTE_FOR: time::Duration = time::Duration::seconds(10);

/// Sets [`WROTE_COOKIE`] on the answer to a request that may have written.
/// Register it at the root before the cookie layer, so it runs inside it and
/// what it adds is written on the response.
pub struct MarkWrites;

impl Layer for MarkWrites {
    fn path(&self) -> Option<&Path> {
        Some(Path::new("/"))
    }

    fn handle<'a>(&'a self, cx: &'a Cx, body: Body, next: Next<'a>) -> LayerFuture<'a> {
        Box::pin(async move {
            let result = next.run(cx, body).await;
            let redirected = match &result {
                Ok(response) => response.status().is_redirection(),
                Err(error) => error.is::<RedirectError>() || error.is::<SeeOther>(),
            };
            if may_have_written(method(cx), redirected) {
                cookies(cx).add(wrote());
            }
            result
        })
    }
}

/// Whether the answer to a request with `method`, a redirect or not, may
/// follow a write: anything but a plain read, and a read that sends the
/// browser on (the callbacks write, then redirect).
fn may_have_written(method: &Method, redirected: bool) -> bool {
    redirected || !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// [`WROTE_COOKIE`], for [`WROTE_FOR`], the whole site, sent on navigation;
/// no script reads it.
fn wrote() -> Cookie<'static> {
    Cookie::build((WROTE_COOKIE, "1"))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(true)
        .max_age(WROTE_FOR)
        .build()
}

/// Whether the browser may have written a moment ago.
pub fn wrote_lately(cx: &Cx) -> bool {
    cookies(cx).get(WROTE_COOKIE).is_some()
}

/// Reads what every request asks first with `read`: from this node's file,
/// asked of the cluster only when `settled` will not let that answer stand
/// ([`LocalReads::first`]); through the cluster at once when the browser
/// wrote a moment ago ([`wrote_lately`]).
pub async fn first<T, F, Fut>(cx: &Cx, read: F, settled: impl FnOnce(&T) -> bool) -> T
where
    F: Fn(Executor) -> Fut,
    Fut: Future<Output = T>,
{
    let store = app_context::<Executor>(cx);
    if wrote_lately(cx) {
        return read(store.clone()).await;
    }
    app_context::<LocalReads>(cx)
        .first(store, read, settled)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_post_or_a_redirect_may_follow_a_write_and_a_plain_read_does_not() {
        assert!(may_have_written(&Method::POST, false));
        assert!(may_have_written(&Method::DELETE, false));
        assert!(may_have_written(&Method::PUT, false));
        // The OIDC and AliExpress callbacks are reads that write, then
        // send the browser on.
        assert!(may_have_written(&Method::GET, true));
        assert!(!may_have_written(&Method::GET, false));
        assert!(!may_have_written(&Method::HEAD, false));
        assert!(!may_have_written(&Method::OPTIONS, false));
    }

    #[test]
    fn the_cookie_lasts_moments_and_no_script_reads_it() {
        let cookie = wrote();
        assert_eq!(cookie.name(), WROTE_COOKIE);
        assert_eq!(cookie.max_age(), Some(WROTE_FOR));
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.path(), Some("/"));
        assert_eq!(cookie.same_site(), Some(SameSite::Lax));
    }
}
