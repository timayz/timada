use std::collections::HashMap;

use timada_storefront::{add_to_cart, cart, require_product};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, route,
    },
};

use super::{back_url, drawer_url};
use crate::shopper::{cart_id, set_cart_id};

/// The form field of an option's menu: `option:<option id>`.
pub const OPTION_FIELD_PREFIX: &str = "option:";

/// What the checkout says when an offer added at its card no longer prices
/// the item: it ended, or the cart stopped meeting it, meanwhile.
pub const OFFER_ENDED: &str = "offer_ended";

/// Adds what the product page's form names: `slug`, `quantity`, and one
/// `option:<id>` per menu, which pick the variant (or `variant_id`, from a
/// form that knows it already). The shopper comes back to `from` (the
/// product page when absent) with the cart drawer open; a refusal goes back
/// to the product page. A form sent from elsewhere says so in `back`
/// (`checkout` for an offer's card), and the shopper comes back there; with
/// `offer`, an offer that no longer prices the item is told.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let field = |name: &str| form.get(name).map(String::as_str).unwrap_or_default();
    let slug = field("slug").to_owned();
    let product = require_product(cx, &slug).await?;
    let quantity = field("quantity").trim().parse::<u32>().unwrap_or(0);
    let picked: Vec<(String, String)> = form
        .iter()
        .filter_map(|(field, value)| {
            field
                .strip_prefix(OPTION_FIELD_PREFIX)
                .map(|option| (option.to_owned(), value.clone()))
        })
        .collect();
    // A product without options sells itself; otherwise the menus name
    // one variant, or none the shop sells (refused by the store).
    let variant_id = if product.options.is_empty() {
        None
    } else if !field("variant_id").is_empty() {
        Some(field("variant_id").to_owned())
    } else {
        Some(
            product
                .variant(&picked)
                .map(|variant| variant.id.clone())
                .unwrap_or_default(),
        )
    };
    let current = cart_id(cx);
    let outcome = add_to_cart(
        cx,
        current.as_deref(),
        &product.id,
        variant_id.as_deref(),
        quantity,
    )
    .await?;
    let back = field("back");
    match outcome {
        Ok(cart_id) => {
            if current.as_deref() != Some(cart_id.as_str()) {
                set_cart_id(cx, &cart_id)?;
            }
            if !back.is_empty() {
                let offer = field("offer");
                let ended = !offer.is_empty()
                    && !cart(cx, &cart_id).await?.is_some_and(|cart| {
                        cart.lines.iter().any(|line| {
                            line.offer
                                .as_ref()
                                .is_some_and(|applied| applied.offer_id == offer)
                        })
                    });
                let error = ended.then_some(OFFER_ENDED);
                return Ok(see_other(back_url(cx, back, field("from"), error)));
            }
            let product_page =
                href!(crate::app::p::slug::index, crate::app::p::slug::Slug(slug)).resolve(cx);
            let from = Some(field("from"))
                .filter(|from| !from.is_empty())
                .unwrap_or(&product_page);
            Ok(see_other(drawer_url(from, None)))
        }
        Err(err) if !back.is_empty() => {
            Ok(see_other(back_url(cx, back, field("from"), err.code())))
        }
        Err(err) => {
            #[derive(serde::Serialize)]
            struct Link<'a> {
                error: Option<&'a str>,
            }
            let back = href!(crate::app::p::slug::index, crate::app::p::slug::Slug(slug))
                .query(Link { error: err.code() })
                .resolve(cx);
            Ok(see_other(back))
        }
    }
}
