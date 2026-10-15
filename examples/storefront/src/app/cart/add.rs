use std::collections::HashMap;

use timada_storefront::{add_to_cart, require_product};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, route,
    },
};

use super::drawer_url;
use crate::shopper::{cart_id, set_cart_id};

/// The form field of an option's menu: `option:<option id>`.
pub const OPTION_FIELD_PREFIX: &str = "option:";

/// Adds what the product page's form names: `slug`, `quantity`, and one
/// `option:<id>` per menu, which pick the variant. The shopper comes back to
/// `from` (the product page when absent) with the cart drawer open; a
/// refusal goes back to the product page.
#[route(POST)]
pub async fn add(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let slug = form.get("slug").cloned().unwrap_or_default();
    let product = require_product(cx, &slug).await?;
    let quantity = form
        .get("quantity")
        .and_then(|q| q.trim().parse::<u32>().ok())
        .unwrap_or(0);
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
    match outcome {
        Ok(cart_id) => {
            if current.as_deref() != Some(cart_id.as_str()) {
                set_cart_id(cx, &cart_id)?;
            }
            let product_page =
                href!(crate::app::p::slug::index, crate::app::p::slug::Slug(slug)).resolve(cx);
            let from = form
                .get("from")
                .filter(|from| !from.is_empty())
                .unwrap_or(&product_page);
            Ok(see_other(drawer_url(from, None)))
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
