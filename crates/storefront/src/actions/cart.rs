//! The cart's actions: add a product, set a line's quantity, take a line
//! out, put a discount code on or off.

use std::collections::HashMap;

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::content::Form;
use topcoat_router::error::{SeeOther, see_other};

use crate::cart::{
    add_to_cart, apply_discount, cart, remove_discount, remove_from_cart, set_cart_quantity,
};
use crate::context::require_product;
use crate::session::{cart_id, set_cart_id};
use crate::storefront::paths;
use crate::urls::{Back, back_url, drawer_url};

use super::action;

/// The form field of an option's menu: `option:<option id>`.
pub const OPTION_FIELD_PREFIX: &str = "option:";

/// What the checkout says when an offer added at its card no longer prices
/// the item: it ended, or the cart stopped meeting it, meanwhile.
pub const OFFER_ENDED: &str = "offer_ended";

/// What the add-to-cart form names: `slug`, `quantity`, one `option:<id>`
/// per menu (or `variant_id` from a form that knows it), `from`, `back`,
/// `offer`. Read as a map, since the option fields are named by the product.
pub type AddForm = HashMap<String, String>;

/// Adds what the product page's form names: `slug`, `quantity`, and one
/// `option:<id>` per menu, which pick the variant (or `variant_id`, from a
/// form that knows it already). The shopper comes back to `from` (the
/// product page when absent) with the cart drawer open; a refusal goes back
/// to the product page. A form sent from elsewhere says so in `back`
/// (`checkout` for an offer's card), and the shopper comes back there; with
/// `offer`, an offer that no longer prices the item is told.
pub(crate) async fn add(cx: &Cx, Form(form): Form<AddForm>) -> Result<SeeOther> {
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
                return Ok(see_other(back_url(
                    cx,
                    Back::parse(back),
                    field("from"),
                    error,
                )));
            }
            let product_page = paths(cx).product_url(&slug, None);
            let from = Some(field("from"))
                .filter(|from| !from.is_empty())
                .unwrap_or(&product_page);
            Ok(see_other(drawer_url(from, None)))
        }
        Err(err) if !back.is_empty() => Ok(see_other(back_url(
            cx,
            Back::parse(back),
            field("from"),
            err.code(),
        ))),
        Err(err) => Ok(see_other(paths(cx).product_url(&slug, err.code()))),
    }
}

/// A line named by a form: the product, and the variant when it has one
/// (an empty field means none).
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LineForm {
    pub product_id: String,
    #[serde(default)]
    pub variant_id: String,
    #[serde(default)]
    pub quantity: Option<u32>,
    /// `checkout` when sent from the checkout's summary, `drawer` from the
    /// cart drawer.
    #[serde(default)]
    pub back: String,
    /// The page the drawer sat on.
    #[serde(default)]
    pub from: String,
}

impl LineForm {
    pub fn variant_id(&self) -> Option<&str> {
        Some(self.variant_id.as_str()).filter(|id| !id.is_empty())
    }

    fn back(&self) -> Back {
        Back::parse(&self.back)
    }
}

/// Sets a line's quantity; zero takes it out.
pub(crate) async fn quantity(cx: &Cx, Form(form): Form<LineForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(paths(cx).cart_url(None)));
    };
    let quantity = form.quantity.unwrap_or(0);
    let outcome =
        set_cart_quantity(cx, &cart_id, &form.product_id, form.variant_id(), quantity).await?;
    let error = outcome.err().and_then(|err| err.code());
    Ok(see_other(back_url(cx, form.back(), &form.from, error)))
}

/// Takes a line out of the cart.
pub(crate) async fn remove(cx: &Cx, Form(form): Form<LineForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(paths(cx).cart_url(None)));
    };
    let outcome = remove_from_cart(cx, &cart_id, &form.product_id, form.variant_id()).await?;
    let error = outcome.err().and_then(|err| err.code());
    Ok(see_other(back_url(cx, form.back(), &form.from, error)))
}

/// What the discount form names.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DiscountForm {
    #[serde(default)]
    pub code: String,
    /// Set by the button that takes the code off.
    #[serde(default)]
    pub remove: Option<String>,
    /// `checkout` when sent from the checkout's summary, `drawer` from the
    /// cart drawer.
    #[serde(default)]
    pub back: String,
    /// The page the drawer sat on.
    #[serde(default)]
    pub from: String,
}

/// Puts a discount code on the cart, or takes it off.
pub(crate) async fn discount(cx: &Cx, Form(form): Form<DiscountForm>) -> Result<SeeOther> {
    let Some(cart_id) = cart_id(cx) else {
        return Ok(see_other(paths(cx).cart_url(None)));
    };
    let outcome = match form.remove {
        Some(_) => remove_discount(cx, &cart_id).await?,
        None => apply_discount(cx, &cart_id, &form.code).await?,
    };
    let error = outcome.err().and_then(|err| err.code());
    Ok(see_other(back_url(
        cx,
        Back::parse(&form.back),
        &form.from,
        error,
    )))
}

action!(add_route = add, Form<AddForm>);
action!(quantity_route = quantity, Form<LineForm>);
action!(remove_route = remove, Form<LineForm>);
action!(discount_route = discount, Form<DiscountForm>);
