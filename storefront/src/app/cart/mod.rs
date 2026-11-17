//! `/cart`: what this browser is about to buy, priced from the store as it
//! stands, with the forms that change it.

pub mod add;
pub mod discount;
pub mod drawer;
pub mod quantity;
pub mod remove;

use timada_storefront::{CartLineView, CartView, cart, menu};
use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::words::t;
use crate::app::{delivery_days, document, money, say};
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

/// Where a cart form sends the shopper back: the checkout when it was sent
/// from there (`back=checkout`), the page the drawer sat on (`back=drawer`,
/// `from`), the cart otherwise. `from` is a path on this site, so a form
/// cannot send anyone elsewhere.
pub fn back_url(cx: &Cx, back: &str, from: &str, error: Option<&str>) -> String {
    match back {
        "checkout" => crate::app::checkout::checkout_url(cx, error),
        "drawer" => drawer_url(from, error),
        _ => cart_url(cx, error),
    }
}

/// `from` when it is a path on this site, without its fragment; the home
/// page for anything else (`//host`, `/\\host`, a full URL), so a form cannot
/// send anyone elsewhere.
pub fn site_path(from: &str) -> &str {
    let from = from.split('#').next().unwrap_or_default();
    if from.starts_with('/') && !from.starts_with("//") && !from.starts_with("/\\") {
        from
    } else {
        "/"
    }
}

/// `from` with the drawer open: `#cart`, and the refusal to show in it as
/// `cart_error`. Anything but a path on this site (`//host`, `/\host`, a
/// full URL) is the home page; refusals it already carried are dropped.
pub fn drawer_url(from: &str, error: Option<&str>) -> String {
    let from = site_path(from);
    let (path, query) = from.split_once('?').unwrap_or((from, ""));
    let mut pairs: Vec<String> = query
        .split('&')
        .filter(|pair| {
            !pair.is_empty() && !pair.starts_with("cart_error=") && !pair.starts_with("error=")
        })
        .map(str::to_owned)
        .collect();
    if let Some(code) = error {
        // A code is snake_case: nothing to escape.
        pairs.push(format!("cart_error={code}"));
    }
    if pairs.is_empty() {
        format!("{path}#cart")
    } else {
        format!("{path}?{}#cart", pairs.join("&"))
    }
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
    let nav = menu(cx).await?;
    let cart = current_cart(cx).await?.filter(|cart| !cart.is_empty());
    let currency = cart
        .as_ref()
        .and_then(|cart| cart.currency.clone())
        .unwrap_or_default();
    let title = t!(cart_title).resolve(cx);

    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            no_drawer: true,
            <div class="wrap">
                <h1>(t!(cart_title))</h1>
                if let Some(code) = query.error.as_deref() {
                    <p class="notice" role="alert">(say(code))</p>
                }
                if let Some(cart) = &cart {
                    <table class="cart" data-cart="">
                        <thead>
                            <tr><th></th><th>(t!(cart_column_product))</th><th>(t!(cart_column_price))</th><th>(t!(cart_column_quantity))</th><th>(t!(cart_column_amount))</th><th></th></tr>
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
                                            <a href=(href!(crate::app::p::slug::index, crate::app::p::slug::slug: slug.clone()))>(&line.name)</a>
                                        } else if line.name.is_empty() {
                                            (t!(cart_line_unnamed))
                                        } else {
                                            (&line.name)
                                        }
                                        if let Some(label) = &line.variant_label {
                                            <div><small>(label)</small></div>
                                        }
                                        if let Some(offer) = &line.offer {
                                            <div><small class="offer-note" data-line-offer=(&offer.offer_id)>(t!(cart_line_offer, price: money(offer.unit_amount_cents as i64, &currency)))</small></div>
                                        }
                                        if let Some(days) = line.delivery_days {
                                            <div><small class="delivery-note" data-line-delivery=(delivery_days(days))>(t!(cart_line_delivery, days: delivery_days(days).resolve(cx)))</small></div>
                                        }
                                        if line.unit_amount_cents.is_none() {
                                            <div><small>(t!(cart_line_off_sale))</small></div>
                                        }
                                    </td>
                                    <td>
                                        if let Some(unit) = line.unit_amount_cents {
                                            (money(unit as i64, &currency))
                                        }
                                    </td>
                                    <td>
                                        <form method="post" action=(href!(quantity::quantity))>
                                            <input type="hidden" name="product_id" value=(&line.product_id)>
                                            <input type="hidden" name="variant_id" value=(line.variant_id.as_deref().unwrap_or(""))>
                                            <label>
                                                <span hidden="">(t!(cart_quantity_of, name: &line.name))</span>
                                                <input type="number" name="quantity" min="0" max="99" value=(line.quantity) aria-label=(t!(cart_quantity_of, name: &line.name))>
                                            </label>
                                            <button>(t!(cart_update))</button>
                                        </form>
                                    </td>
                                    <td data-line-amount="">
                                        if let Some(amount) = line.amount_cents {
                                            (money(amount as i64, &currency))
                                        }
                                    </td>
                                    <td>
                                        <form method="post" action=(href!(remove::remove))>
                                            <input type="hidden" name="product_id" value=(&line.product_id)>
                                            <input type="hidden" name="variant_id" value=(line.variant_id.as_deref().unwrap_or(""))>
                                            <button aria-label=(t!(cart_remove.label, name: &line.name))>(t!(cart_remove))</button>
                                        </form>
                                    </td>
                                </tr>
                            }
                        </tbody>
                    </table>
                    <p>(t!(cart_subtotal_label)) " " <span data-cart-subtotal="">(cart.subtotal().unwrap_or_default())</span></p>
                    if let Some(days) = cart.delivery_window {
                        <p data-cart-delivery=(delivery_days(days))>(t!(cart_delivery_estimate, days: delivery_days(days).resolve(cx)))</p>
                    }
                    if let (Some(discount), Some(amount)) = (&cart.discount, cart.discount_amount()) {
                        <p data-cart-discount=(&discount.code)>(t!(cart_promo_code)) " " <code>(&discount.code)</code> (t!(cart_promo_amount_after)) " " <span data-discount-amount="">(amount)</span></p>
                    }
                    if let Some(refused) = cart.discount_refused {
                        <p class="notice" role="alert" data-discount-refused=(refused)>(say(refused))</p>
                    }
                    <form class="discount" method="post" action=(href!(discount::discount))>
                        if let Some(code) = &cart.discount_code {
                            <span>(t!(cart_promo_applied)) " " <code>(code)</code></span>
                            <button name="remove" value="1">(t!(cart_promo_remove))</button>
                        } else {
                            <label>
                                (t!(cart_promo_code)) " "
                                <input type="text" name="code" maxlength="32" autocomplete="off">
                            </label>
                            <button>(t!(cart_promo_apply))</button>
                        }
                    </form>
                    <p>(t!(cart_total_label)) " " <strong class="price" data-cart-total="">(cart.total().unwrap_or_default())</strong></p>
                    if cart.can_check_out() {
                        <p><a class="btn" href=(href!(crate::app::checkout::index))>(t!(cart_order))</a></p>
                    }
                } else {
                    <p data-cart-empty="">(t!(cart_empty))</p>
                    <p><a href=(href!(crate::app::index))>(t!(cart_continue))</a></p>
                }
            </div>
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
}

/// One line of a cart beside the checkout or in the drawer: the picture,
/// the name, a −/+ stepper and the amount with a remove button. Each button
/// is its own small form, so it works without the page's script; `back` and
/// `from` say where it comes back to.
#[topcoat::view::component]
pub async fn summary_line(
    cx: &Cx,
    line: &CartLineView,
    currency: &str,
    back: &str,
    #[default] from: &str,
) -> Result<impl View> {
    let variant_id = line.variant_id.clone().unwrap_or_default();
    let name = if line.name.is_empty() {
        t!(cart_line_unnamed).resolve(cx).into_owned()
    } else {
        line.name.clone()
    };
    Ok(view! {
        <li data-summary-line=(&line.name) data-available=(if line.amount_cents.is_some() { "true" } else { "false" })>
            <span class="pic">
                if let Some(url) = &line.image_url {
                    <img src=(url) alt="" width="56" height="56" loading="lazy">
                }
                <span class="badge" aria-hidden="true">(line.quantity)</span>
            </span>
            <span class="what">
                if let Some(slug) = &line.slug {
                    <a href=(href!(crate::app::p::slug::index, crate::app::p::slug::slug: slug.clone()))>(&name)</a>
                } else {
                    (&name)
                }
                if let Some(label) = &line.variant_label { <small class="variant">(label)</small> }
                if let Some(offer) = &line.offer {
                    <small class="offer-note" data-line-offer=(&offer.offer_id)>(t!(cart_line_offer, price: money(offer.unit_amount_cents as i64, currency)))</small>
                }
                if let Some(days) = line.delivery_days {
                    <small class="delivery-note" data-line-delivery=(delivery_days(days))>(t!(cart_line_delivery, days: delivery_days(days).resolve(cx)))</small>
                }
                if line.amount_cents.is_some() {
                    <span class="qty">
                        <form method="post" action=(href!(quantity::quantity))>
                            <input type="hidden" name="product_id" value=(&line.product_id)>
                            <input type="hidden" name="variant_id" value=(&variant_id)>
                            <input type="hidden" name="quantity" value=(line.quantity.saturating_sub(1))>
                            <input type="hidden" name="back" value=(back)>
                            <input type="hidden" name="from" value=(from)>
                            <button aria-label=(t!(cart_one_less, name: &line.name))>"−"</button>
                        </form>
                        <span data-line-quantity="">(line.quantity)</span>
                        <form method="post" action=(href!(quantity::quantity))>
                            <input type="hidden" name="product_id" value=(&line.product_id)>
                            <input type="hidden" name="variant_id" value=(&variant_id)>
                            <input type="hidden" name="quantity" value=(line.quantity + 1)>
                            <input type="hidden" name="back" value=(back)>
                            <input type="hidden" name="from" value=(from)>
                            <button aria-label=(t!(cart_one_more, name: &line.name)) disabled=(line.quantity >= 99)>"+"</button>
                        </form>
                    </span>
                } else {
                    <small class="off-sale">(t!(cart_line_off_sale))</small>
                }
            </span>
            <span class="amount">
                if let Some(amount) = line.amount_cents {
                    (money(amount as i64, currency))
                }
                <form method="post" action=(href!(remove::remove))>
                    <input type="hidden" name="product_id" value=(&line.product_id)>
                    <input type="hidden" name="variant_id" value=(&variant_id)>
                    <input type="hidden" name="back" value=(back)>
                    <input type="hidden" name="from" value=(from)>
                    <button class="remove" aria-label=(t!(cart_remove.label, name: &line.name))>"×"</button>
                </form>
            </span>
        </li>
    })
}
