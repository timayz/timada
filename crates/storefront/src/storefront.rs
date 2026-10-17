//! What the app hands the router once: the store, the key its shopper
//! cookies are sealed with, and the things the library has to know about
//! the app to act on its behalf (its wording, where its pages are, how it
//! writes money).

use std::borrow::Cow;
use std::sync::Arc;

use topcoat_cookie::{Key, SameSite};
use topcoat_core::context::{Cx, app_context};
use topcoat_router::IpNet;

use crate::messages::Messages;
use crate::money::MoneyFormat;
use crate::paths::Paths;
use crate::store::Store;

/// How the shopper cookies (the cart, the orders placed, the last address,
/// the shopper's session) are set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CookieOptions {
    /// `Secure`: a shop behind HTTPS sets it (the default is off, since the
    /// examples serve plain HTTP on 127.0.0.1).
    pub secure: bool,
    pub same_site: SameSite,
    pub http_only: bool,
    pub path: Cow<'static, str>,
    /// How long a browser keeps its cart, its orders and its address.
    pub max_age_days: i64,
    /// How long a sign-in on its way is remembered.
    pub sign_in_minutes: i64,
    /// Before every cookie's name (`__Host-`, say, with `secure`).
    pub name_prefix: Cow<'static, str>,
}

impl Default for CookieOptions {
    fn default() -> Self {
        Self {
            secure: false,
            same_site: SameSite::Lax,
            http_only: true,
            path: "/".into(),
            max_age_days: 30,
            sign_in_minutes: 15,
            name_prefix: "".into(),
        }
    }
}

/// The URLs of the actions the library serves, under the store's prefix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actions {
    prefix: String,
}

macro_rules! action_url {
    ($($name:ident => $path:literal),* $(,)?) => {
        $(
            pub fn $name(&self) -> String {
                format!("{}{}", self.prefix, $path)
            }
        )*
    };
}

impl Actions {
    pub(crate) fn new(prefix: &str) -> Self {
        Self {
            prefix: prefix.trim_end_matches('/').to_owned(),
        }
    }

    action_url! {
        cart_add => "/cart/add",
        cart_quantity => "/cart/quantity",
        cart_remove => "/cart/remove",
        cart_discount => "/cart/discount",
        checkout_start => "/checkout/start",
        checkout_place => "/checkout/place",
        sign_in_request => "/sign-in",
        sign_in_verify => "/sign-in/verify",
        sign_out => "/sign-out",
        sign_out_everywhere => "/sign-out-everywhere",
        account_delete => "/account/delete",
        address_remove => "/addresses/remove",
        review_submit => "/reviews/submit",
        review_revise => "/reviews/revise",
    }

    /// The route pattern of the PDF of an order's document.
    pub(crate) fn document_pdf_route(&self) -> String {
        format!(
            "{}/orders/{{order_id}}/documents/{{document_id}}",
            self.prefix
        )
    }

    /// One of an order's documents, as PDF.
    pub fn document_pdf(&self, order_id: &str, document_id: &str) -> String {
        format!(
            "{}/orders/{}/documents/{}",
            self.prefix,
            crate::urls::encode(order_id),
            crate::urls::encode(document_id)
        )
    }
}

/// The store and everything the library acts with on the app's behalf.
/// Built once and handed to
/// [`RouterBuilderStoreExt::store`](crate::RouterBuilderStoreExt::store).
#[derive(Clone)]
pub struct Storefront {
    pub(crate) store: Store,
    pub(crate) cookie_key: Option<Key>,
    pub(crate) cookies: CookieOptions,
    pub(crate) messages: Arc<Messages>,
    pub(crate) paths: Arc<Paths>,
    pub(crate) money: MoneyFormat,
    pub(crate) seller_country: Option<String>,
    pub(crate) accent_color: Option<String>,
    pub(crate) trusted_proxies: Vec<IpNet>,
    pub(crate) review_body_limit: usize,
    pub(crate) install_cookie_layer: bool,
    pub(crate) actions: Actions,
}

/// A review carries up to three photos of up to 8 MiB each.
const REVIEW_BODY_LIMIT: usize = 26 * 1024 * 1024;

impl Storefront {
    /// A storefront that shows the catalog and serves `GET {prefix}/status`.
    /// With [`cookie_key`](Self::cookie_key) it also sells: the cart,
    /// checkout, account and review actions are served under the prefix.
    pub fn new(store: Store) -> Self {
        let actions = Actions::new(store.prefix());
        Self {
            store,
            cookie_key: None,
            cookies: CookieOptions::default(),
            messages: Arc::new(Messages::default()),
            paths: Arc::new(Paths::default()),
            money: MoneyFormat::default(),
            seller_country: None,
            accent_color: None,
            trusted_proxies: Vec::new(),
            review_body_limit: REVIEW_BODY_LIMIT,
            install_cookie_layer: true,
            actions,
        }
    }

    /// The key the shopper cookies are sealed with; every replica of the
    /// app shares it. Setting it turns the actions on.
    pub fn cookie_key(mut self, key: Key) -> Self {
        self.cookie_key = Some(key);
        self
    }

    pub fn cookies(mut self, options: CookieOptions) -> Self {
        self.cookies = options;
        self
    }

    /// The shop's wording; English unless set.
    pub fn messages(mut self, messages: Messages) -> Self {
        self.messages = Arc::new(messages);
        self
    }

    /// Where the app's pages are; the conventional paths unless set.
    pub fn paths(mut self, paths: Paths) -> Self {
        self.paths = Arc::new(paths);
        self
    }

    /// How the pages write money; `12.50 EUR` unless set.
    pub fn money(mut self, format: MoneyFormat) -> Self {
        self.money = format;
        self
    }

    /// The merchant's country (ISO 3166-1 alpha-2): a business buyer from
    /// another country with a VAT number is billed without VAT, so the
    /// checkout hides the VAT line then. Unset, any VAT number hides it.
    pub fn seller_country(mut self, code: impl Into<String>) -> Self {
        self.seller_country = Some(code.into().to_ascii_uppercase());
        self
    }

    /// The colour the payment provider's form takes (`#f46063`), if the
    /// shop wants one.
    pub fn accent_color(mut self, css: impl Into<String>) -> Self {
        self.accent_color = Some(css.into());
        self
    }

    /// The reverse proxies in front of the app, whose `X-Forwarded-For`
    /// names the shopper (the dashboard limits the sign-in codes each
    /// address may ask for).
    pub fn trusted_proxies(mut self, networks: Vec<IpNet>) -> Self {
        self.trusted_proxies = networks;
        self
    }

    /// How large a review with its photos may be.
    pub fn review_body_limit(mut self, bytes: usize) -> Self {
        self.review_body_limit = bytes;
        self
    }

    /// Whether the router gets a cookie layer from the library (the
    /// default). An app that installs its own `.cookies()` turns this off.
    pub fn cookie_layer(mut self, install: bool) -> Self {
        self.install_cookie_layer = install;
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn actions(&self) -> &Actions {
        &self.actions
    }

    /// Whether the actions are served: a cookie key was given.
    pub fn sells(&self) -> bool {
        self.cookie_key.is_some()
    }
}

impl From<Store> for Storefront {
    fn from(store: Store) -> Self {
        Self::new(store)
    }
}

/// What the app handed the router.
///
/// # Panics
///
/// Panics when none was registered.
pub fn storefront(cx: &Cx) -> &Storefront {
    app_context::<Storefront>(cx)
}

/// The shop's wording.
pub fn messages(cx: &Cx) -> &Messages {
    &storefront(cx).messages
}

/// Where the app's pages are.
pub fn paths(cx: &Cx) -> &Paths {
    &storefront(cx).paths
}

/// How the pages write money.
pub fn money_format(cx: &Cx) -> &MoneyFormat {
    &storefront(cx).money
}

/// The URLs of the actions the library serves.
pub fn actions(cx: &Cx) -> &Actions {
    &storefront(cx).actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_urls_sit_under_the_prefix() {
        let actions = Actions::new("/store");
        assert_eq!(actions.cart_add(), "/store/cart/add");
        assert_eq!(
            actions.document_pdf("o 1", "d1"),
            "/store/orders/o%201/documents/d1"
        );
        assert_eq!(
            actions.document_pdf_route(),
            "/store/orders/{order_id}/documents/{document_id}"
        );
        assert_eq!(Actions::new("/shop/").sign_out(), "/shop/sign-out");
    }
}
