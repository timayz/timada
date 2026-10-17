//! Registration into the host's router: the store in the app context, the
//! status endpoint under the host's prefix, and (with a cookie key) the
//! shopper cookies and the actions. Hand-written routes, never `#[route]`:
//! a library's paths are the host's to choose.

use topcoat_cookie::{CookieLayer, Key};
use topcoat_core::context::Cx;
use topcoat_router::{
    Body, BodyLimit, HeaderValue, Method, Methods, Path, PathBuf, Route, RouteFuture, RouteId,
    RouterBuilder, StatusCode, TrustedProxies,
    header::{CACHE_CONTROL, CONTENT_TYPE},
    response::Response,
};

use crate::store::Store;
use crate::storefront::Storefront;

pub trait RouterBuilderStoreExt {
    /// Puts the store in the app context and serves `GET {prefix}/status`.
    /// Given a [`Storefront`] with a cookie key, also installs the shopper
    /// cookies, the trusted proxies, and the cart, checkout, account and
    /// review actions under the prefix.
    #[must_use]
    fn store(self, storefront: impl Into<Storefront>) -> Self;
}

impl RouterBuilderStoreExt for RouterBuilder {
    fn store(mut self, storefront: impl Into<Storefront>) -> Self {
        let storefront = storefront.into();
        let store = storefront.store.clone();
        self = self.route(StatusRoute::new(&store));
        if let Some(key) = &storefront.cookie_key {
            if storefront.install_cookie_layer {
                self = self.layer(CookieLayer::new());
            }
            if self.get_app_context::<Key>().is_none() {
                self = self.app_context(key.clone());
            }
            if !storefront.trusted_proxies.is_empty() {
                self = self.trusted_proxies(
                    TrustedProxies::new().networks(storefront.trusted_proxies.iter().copied()),
                );
            }
            let reviews = format!("{}/reviews", store.prefix().trim_end_matches('/'));
            self = self.layer(
                BodyLimit::max(storefront.review_body_limit).at(Path::new(&reviews).to_owned()),
            );
            self = crate::actions::register(self, &storefront);
        }
        // A second registration of the same type would panic.
        if self.get_app_context::<Store>().is_none() {
            self = self.app_context(store);
        }
        if self.get_app_context::<Storefront>().is_none() {
            self = self.app_context(storefront);
        }
        self
    }
}

/// `GET {prefix}/status`: the store's health as JSON, for a load balancer or
/// an operator (see [`StoreStatus`](crate::StoreStatus)).
pub struct StatusRoute {
    id: RouteId,
    path: PathBuf,
    store: Store,
}

impl StatusRoute {
    pub fn new(store: &Store) -> Self {
        Self {
            id: RouteId::new(),
            path: Path::new(&format!("{}/status", store.prefix())).to_owned(),
            store: store.clone(),
        }
    }
}

impl Route for StatusRoute {
    fn id(&self) -> RouteId {
        self.id
    }

    fn methods(&self) -> Methods<'_> {
        Methods::Only(&[Method::GET])
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn handle<'cx>(&'cx self, _cx: &'cx Cx, _body: Body) -> RouteFuture<'cx> {
        Box::pin(async move {
            let status = self.store.status().await;
            let body = serde_json::to_string(&status)
                .map_err(|err| topcoat_core::error::Error::from_anyhow(anyhow::Error::new(err)))?;
            let mut response = Response::new(Body::from(body));
            if !status.connected {
                *response.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            }
            let headers = response.headers_mut();
            headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
            Ok(response)
        })
    }
}
