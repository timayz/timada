//! Where the shop's pages live. The library's actions send the shopper back
//! to pages the app serves (a product, the cart, the checkout, an order, the
//! account); the app says where they are, or keeps the defaults.

use std::borrow::Cow;

use crate::query::ProductSort;
use crate::urls::{encode, query};

/// A page's path with one hole: `/p/{slug}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathTemplate(Cow<'static, str>);

impl PathTemplate {
    /// # Panics
    ///
    /// Panics unless `template` has exactly one `{…}` hole: a wrong path is
    /// a mistake in the app's setup, found at startup.
    pub fn new(template: impl Into<Cow<'static, str>>) -> Self {
        let template = template.into();
        let holes = template.matches('{').count();
        assert_eq!(
            holes, 1,
            "a page path takes exactly one {{hole}}, got {template:?}"
        );
        assert!(
            template.contains('}'),
            "a page path's hole must close, got {template:?}"
        );
        Self(template)
    }

    /// The path for `value`, percent-encoded.
    pub fn fill(&self, value: &str) -> String {
        let start = self.0.find('{').unwrap_or(self.0.len());
        let end = self.0[start..]
            .find('}')
            .map(|i| start + i + 1)
            .unwrap_or(self.0.len());
        format!("{}{}{}", &self.0[..start], encode(value), &self.0[end..])
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&'static str> for PathTemplate {
    fn from(template: &'static str) -> Self {
        Self::new(template)
    }
}

impl From<String> for PathTemplate {
    fn from(template: String) -> Self {
        Self::new(template)
    }
}

/// The app's pages, by path. Every field has a conventional default, so an
/// app laid out that way configures nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    pub home: Cow<'static, str>,
    /// The whole catalogue, searched with `?q=`.
    pub catalog: Cow<'static, str>,
    pub product: PathTemplate,
    pub category: PathTemplate,
    pub cart: Cow<'static, str>,
    pub checkout: Cow<'static, str>,
    /// Where the payment provider sends the browser back.
    pub checkout_complete: Cow<'static, str>,
    /// Where the card form used to be: the library answers it with a
    /// redirect to the checkout, for bookmarks and stale returns. `None`
    /// serves nothing there.
    pub checkout_pay: Option<Cow<'static, str>>,
    pub order: PathTemplate,
    pub account: Cow<'static, str>,
    pub account_delete: Cow<'static, str>,
    pub sign_in: Cow<'static, str>,
    pub sign_in_code: Cow<'static, str>,
    /// The shop's own pages (legal, support…).
    pub page: PathTemplate,
    /// Where a review request's mailed link lands.
    pub review_link: PathTemplate,
}

impl Default for Paths {
    fn default() -> Self {
        Self {
            home: "/".into(),
            catalog: "/catalog".into(),
            product: "/p/{slug}".into(),
            category: "/c/{slug}".into(),
            cart: "/cart".into(),
            checkout: "/checkout".into(),
            checkout_complete: "/checkout/complete".into(),
            checkout_pay: Some("/checkout/pay".into()),
            order: "/orders/{id}".into(),
            account: "/account".into(),
            account_delete: "/account/delete".into(),
            sign_in: "/account/sign-in".into(),
            sign_in_code: "/account/sign-in/code".into(),
            page: "/pages/{slug}".into(),
            review_link: "/reviews/{token}".into(),
        }
    }
}

macro_rules! setter {
    ($name:ident: Cow) => {
        pub fn $name(mut self, path: impl Into<Cow<'static, str>>) -> Self {
            self.$name = path.into();
            self
        }
    };
    ($name:ident: PathTemplate) => {
        pub fn $name(mut self, template: impl Into<PathTemplate>) -> Self {
            self.$name = template.into();
            self
        }
    };
}

impl Paths {
    setter!(home: Cow);
    setter!(catalog: Cow);
    setter!(product: PathTemplate);
    setter!(category: PathTemplate);
    setter!(cart: Cow);
    setter!(checkout: Cow);
    setter!(checkout_complete: Cow);
    setter!(order: PathTemplate);
    setter!(account: Cow);
    setter!(account_delete: Cow);
    setter!(sign_in: Cow);
    setter!(sign_in_code: Cow);
    setter!(page: PathTemplate);
    setter!(review_link: PathTemplate);

    /// Where the old card form page was; `None` serves nothing there.
    pub fn checkout_pay(mut self, path: Option<impl Into<Cow<'static, str>>>) -> Self {
        self.checkout_pay = path.map(Into::into);
        self
    }

    pub fn home_url(&self) -> String {
        self.home.to_string()
    }

    /// The catalogue, searched for `q`, at page `page`.
    pub fn catalog_url(&self, q: Option<&str>, page: u32) -> String {
        let page = page.to_string();
        format!(
            "{}{}",
            self.catalog,
            query(&[("q", q), ("page", Some(&page))])
        )
    }

    /// A product's page, with a refusal to show, by code.
    pub fn product_url(&self, slug: &str, error: Option<&str>) -> String {
        format!("{}{}", self.product.fill(slug), query(&[("error", error)]))
    }

    /// A page of a product's reviews, scrolled to them.
    pub fn product_reviews_url(&self, slug: &str, page: u32) -> String {
        let page = page.to_string();
        format!(
            "{}{}#reviews",
            self.product.fill(slug),
            query(&[("reviews", Some(&page))])
        )
    }

    /// A category's page, sorted and paged.
    pub fn category_url(&self, slug: &str, sort: ProductSort, page: u32) -> String {
        let page = page.to_string();
        format!(
            "{}{}",
            self.category.fill(slug),
            query(&[("sort", Some(sort.as_str())), ("page", Some(&page))])
        )
    }

    /// A category's page as a link shows it, unsorted.
    pub fn category_link(&self, slug: &str) -> String {
        self.category.fill(slug)
    }

    /// The cart's page with a refusal to show, by code.
    pub fn cart_url(&self, error: Option<&str>) -> String {
        format!("{}{}", self.cart, query(&[("error", error)]))
    }

    /// The checkout with a refusal to show, by code.
    pub fn checkout_url(&self, error: Option<&str>) -> String {
        format!("{}{}", self.checkout, query(&[("error", error)]))
    }

    /// The checkout, shipping to and billing to saved addresses of the
    /// signed-in shopper.
    pub fn checkout_saved_url(&self, address: Option<&str>, billing: Option<&str>) -> String {
        format!(
            "{}{}",
            self.checkout,
            query(&[("address", address), ("billing", billing)])
        )
    }

    pub fn checkout_complete_url(&self) -> String {
        self.checkout_complete.to_string()
    }

    pub fn order_url(&self, order_id: &str) -> String {
        self.order.fill(order_id)
    }

    /// The order's page, scrolled to its review forms.
    pub fn order_reviews_url(&self, order_id: &str) -> String {
        format!("{}#reviews", self.order.fill(order_id))
    }

    /// The order's page after a review of `product` was handed in (or
    /// changed, `revised`): a refusal by code, or a confirmation; scrolled
    /// to that review.
    pub fn order_review_url(
        &self,
        order_id: &str,
        product: &str,
        error: Option<&str>,
        revised: bool,
    ) -> String {
        let done = error.is_none().then_some("1");
        format!(
            "{}{}#review-{product}",
            self.order.fill(order_id),
            query(&[
                ("product", Some(product)),
                ("error", error),
                ("reviewed", done.filter(|_| !revised)),
                ("revised", done.filter(|_| revised)),
            ])
        )
    }

    /// The account page with a refusal to show, by code.
    pub fn account_url(&self, error: Option<&str>) -> String {
        format!("{}{}", self.account, query(&[("error", error)]))
    }

    /// The account page saying what was just done, by code.
    pub fn account_done_url(&self, done: &str) -> String {
        format!("{}{}", self.account, query(&[("done", Some(done))]))
    }

    pub fn account_delete_url(&self) -> String {
        self.account_delete.to_string()
    }

    /// The sign-in page, coming back to `next` once signed in, with a
    /// refusal to show.
    pub fn sign_in_url(&self, next: Option<&str>, error: Option<&str>) -> String {
        format!(
            "{}{}",
            self.sign_in,
            query(&[("next", next), ("error", error)])
        )
    }

    /// The page the code is entered on, with a refusal to show, or saying a
    /// new code was sent.
    pub fn sign_in_code_url(&self, error: Option<&str>, resent: bool) -> String {
        format!(
            "{}{}",
            self.sign_in_code,
            query(&[("error", error), ("resent", resent.then_some("1"))])
        )
    }

    pub fn page_url(&self, slug: &str) -> String {
        self.page.fill(slug)
    }

    pub fn review_link_url(&self, token: &str) -> String {
        self.review_link.fill(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_fill_their_hole() {
        assert_eq!(
            PathTemplate::new("/p/{slug}").fill("blue mug"),
            "/p/blue%20mug"
        );
        assert_eq!(PathTemplate::new("/{id}/pdf").fill("x"), "/x/pdf");
    }

    #[test]
    #[should_panic(expected = "exactly one")]
    fn a_template_needs_one_hole() {
        PathTemplate::new("/p/slug");
    }

    #[test]
    fn urls_keep_their_shapes() {
        let paths = Paths::default();
        assert_eq!(paths.cart_url(None), "/cart");
        assert_eq!(paths.cart_url(Some("empty_cart")), "/cart?error=empty_cart");
        assert_eq!(
            paths.product_url("mug", Some("variant_required")),
            "/p/mug?error=variant_required"
        );
        assert_eq!(
            paths.product_reviews_url("mug", 2),
            "/p/mug?reviews=2#reviews"
        );
        assert_eq!(
            paths.category_url("toys", ProductSort::PriceAsc, 3),
            "/c/toys?sort=price_asc&page=3"
        );
        assert_eq!(paths.catalog_url(Some("chat"), 1), "/catalog?q=chat&page=1");
        assert_eq!(
            paths.checkout_saved_url(Some("a1"), None),
            "/checkout?address=a1"
        );
        assert_eq!(
            paths.checkout_saved_url(None, Some("b2")),
            "/checkout?billing=b2"
        );
        assert_eq!(
            paths.order_review_url("o1", "p1", None, false),
            "/orders/o1?product=p1&reviewed=1#review-p1"
        );
        assert_eq!(
            paths.order_review_url("o1", "p1", Some("review_invalid_body"), true),
            "/orders/o1?product=p1&error=review_invalid_body#review-p1"
        );
        assert_eq!(
            paths.sign_in_url(Some("/checkout"), None),
            "/account/sign-in?next=%2Fcheckout"
        );
        assert_eq!(
            paths.sign_in_code_url(None, true),
            "/account/sign-in/code?resent=1"
        );
        assert_eq!(paths.account_done_url("x"), "/account?done=x");
        let paths = Paths::default()
            .catalog("/boutique")
            .product("/produit/{slug}");
        assert_eq!(paths.catalog_url(None, 2), "/boutique?page=2");
        assert_eq!(paths.product_url("mug", None), "/produit/mug");
    }
}
