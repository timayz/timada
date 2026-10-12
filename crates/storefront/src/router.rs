//! Registration into the host's router: the store in the app context and the
//! status endpoint under the host's prefix. Hand-written routes, never
//! `#[route]`: a library's paths are the host's to choose.

use topcoat_core::context::Cx;
use topcoat_router::{
    Body, HeaderValue, Method, Methods, Path, PathBuf, Route, RouteFuture, RouteId, RouterBuilder,
    StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE},
    response::Response,
};

use crate::store::Store;

pub trait RouterBuilderStoreExt {
    /// Puts `store` in the app context and serves `GET {prefix}/status`.
    #[must_use]
    fn store(self, store: Store) -> Self;
}

impl RouterBuilderStoreExt for RouterBuilder {
    fn store(mut self, store: Store) -> Self {
        self = self.route(StatusRoute::new(&store));
        // A second registration of the same type would panic.
        if self.get_app_context::<Store>().is_none() {
            self = self.app_context(store);
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
