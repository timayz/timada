//! `/cart`: what this browser is about to buy, priced from the store as it
//! stands, with the forms that change it.

pub mod add;
pub mod discount;
pub mod quantity;
pub mod remove;

use timada_storefront::{CartView, cart, categories, format_money};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::{document, say};
use crate::shopper::cart_id;

#[topcoat::router::query_params(error = bad_request)]
pub(crate) struct CartQuery {
    pub error: Option<String>,
}

/// The cart's page with a refusal to show, by code.
pub fn cart_url(cx: &Cx, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        error: Option<&'a str>,
    }
    href!(index).query(Link { error }).resolve(cx)
}

/// This browser's cart, unless it has none or it became an order.
pub async fn current_cart(cx: &Cx) -> Result<Option<CartView>> {
    let Some(id) = cart_id(cx) else {
        return Ok(None);
    };
    Ok(cart(cx, &id).await?.filter(|cart| cart.order_id.is_none()))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<CartQuery>(cx)?;
    let nav = categories(cx).await?;
    let cart = current_cart(cx).await?.filter(|cart| !cart.is_empty());
    let currency = cart
        .as_ref()
        .and_then(|cart| cart.currency.clone())
        .unwrap_or_default();

    Ok(view! {
        document(
            title: "Your cart",
            nav: &nav,
            <h1>"Your cart"</h1>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            if let Some(cart) = &cart {
                <table class="cart" data-cart="">
                    <thead>
                        <tr><th></th><th>"Product"</th><th>"Price"</th><th>"Quantity"</th><th>"Amount"</th><th></th></tr>
                    </thead>
                    <tbody>
                        for line in cart.lines.iter().chain(&cart.unavailable) {
                            <tr data-cart-line=(&line.name) data-available=(if line.unit_amount_cents.is_some() { "true" } else { "false" })>
                                <td>
                                    if let Some(url) = &line.image_url {
                                        <img src=(url) alt="" loading="lazy">
                                    }
                                </td>
                                <td>
                                    if let Some(slug) = &line.slug {
                                        <a href=(href!(crate::app::p::slug::index, crate::app::p::slug::Slug(slug.clone())))>(&line.name)</a>
                                    } else {
                                        (if line.name.is_empty() { "A product no longer sold" } else { line.name.as_str() })
                                    }
                                    if let Some(label) = &line.variant_label {
                                        <div><small>(label)</small></div>
                                    }
                                    if line.unit_amount_cents.is_none() {
                                        <div><small>"No longer on sale: remove it to order the rest."</small></div>
                                    }
                                </td>
                                <td>
                                    if let Some(unit) = line.unit_amount_cents {
                                        (format_money(unit, &currency))
                                    }
                                </td>
                                <td>
                                    <form method="post" action=(href!(quantity::quantity))>
                                        <input type="hidden" name="product_id" value=(&line.product_id)>
                                        <input type="hidden" name="variant_id" value=(line.variant_id.as_deref().unwrap_or(""))>
                                        <label>
                                            <span hidden="">"Quantity of " (&line.name)</span>
                                            <input type="number" name="quantity" min="0" max="99" value=(line.quantity) aria-label=(format!("Quantity of {}", line.name))>
                                        </label>
                                        <button>"Update"</button>
                                    </form>
                                </td>
                                <td data-line-amount="">
                                    if let Some(amount) = line.amount_cents {
                                        (format_money(amount, &currency))
                                    }
                                </td>
                                <td>
                                    <form method="post" action=(href!(remove::remove))>
                                        <input type="hidden" name="product_id" value=(&line.product_id)>
                                        <input type="hidden" name="variant_id" value=(line.variant_id.as_deref().unwrap_or(""))>
                                        <button aria-label=(format!("Remove {}", line.name))>"Remove"</button>
                                    </form>
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
                <p>"Subtotal: " <span data-cart-subtotal="">(cart.subtotal().unwrap_or_default())</span></p>
                if let (Some(discount), Some(amount)) = (&cart.discount, cart.discount_amount()) {
                    <p data-cart-discount=(&discount.code)>"Discount " <code>(&discount.code)</code> ": " <span data-discount-amount="">(amount)</span></p>
                }
                if let Some(refused) = cart.discount_refused {
                    <p class="notice" role="alert" data-discount-refused=(refused)>(say(refused))</p>
                }
                <form class="discount" method="post" action=(href!(discount::discount))>
                    if let Some(code) = &cart.discount_code {
                        <span>"Code " <code>(code)</code></span>
                        <button name="remove" value="1">"Remove the code"</button>
                    } else {
                        <label>
                            "Discount code "
                            <input type="text" name="code" maxlength="32" autocomplete="off">
                        </label>
                        <button>"Apply"</button>
                    }
                </form>
                <p>"Total: " <strong class="price" data-cart-total="">(cart.total().unwrap_or_default())</strong></p>
                if cart.can_check_out() {
                    <p><a href=(href!(crate::app::checkout::index))>"Check out"</a></p>
                }
            } else {
                <p data-cart-empty="">"Your cart is empty."</p>
                <p><a href=(href!(crate::app::index))>"Back to the catalog"</a></p>
            }
        )
    })
}

/// A line named by a form: the product, and the variant when it has one
/// (an empty field means none).
#[derive(serde::Deserialize)]
pub struct LineForm {
    pub product_id: String,
    #[serde(default)]
    pub variant_id: String,
    #[serde(default)]
    pub quantity: Option<u32>,
}

impl LineForm {
    pub fn variant_id(&self) -> Option<&str> {
        Some(self.variant_id.as_str()).filter(|id| !id.is_empty())
    }
}
