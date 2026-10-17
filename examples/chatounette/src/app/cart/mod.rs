//! `/cart`: what this browser is about to buy, priced from the store as it
//! stands, with the forms that change it.

pub mod add;
pub mod discount;
pub mod drawer;
pub mod quantity;
pub mod remove;

use timada_storefront::{CartLineView, CartView, cart, format_money, menu};
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

/// `from` with the drawer open: `#cart`, and the refusal to show in it as
/// `cart_error`. Anything but a path on this site (`//host`, `/\host`, a
/// full URL) is the home page; refusals it already carried are dropped.
pub fn drawer_url(from: &str, error: Option<&str>) -> String {
    let from = from.split('#').next().unwrap_or_default();
    let from = if from.starts_with('/') && !from.starts_with("//") && !from.starts_with("/\\") {
        from
    } else {
        "/"
    };
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

    Ok(view! {
        document(
            title: "Votre panier",
            nav: &nav,
            no_drawer: true,
            <h1>"Votre panier"</h1>
            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            if let Some(cart) = &cart {
                <table class="cart" data-cart="">
                    <thead>
                        <tr><th></th><th>"Produit"</th><th>"Prix"</th><th>"Quantité"</th><th>"Montant"</th><th></th></tr>
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
                                        (if line.name.is_empty() { "Un produit qui n'est plus vendu" } else { line.name.as_str() })
                                    }
                                    if let Some(label) = &line.variant_label {
                                        <div><small>(label)</small></div>
                                    }
                                    if let Some(offer) = &line.offer {
                                        <div><small class="offer-note" data-line-offer=(&offer.offer_id)>"Offre : 1 à " (format_money(offer.unit_amount_cents, &currency))</small></div>
                                    }
                                    if line.unit_amount_cents.is_none() {
                                        <div><small>"Plus en vente : retirez-le pour commander le reste."</small></div>
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
                                            <span hidden="">"Quantité de " (&line.name)</span>
                                            <input type="number" name="quantity" min="0" max="99" value=(line.quantity) aria-label=(format!("Quantité de {}", line.name))>
                                        </label>
                                        <button>"Mettre à jour"</button>
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
                                        <button aria-label=(format!("Retirer {}", line.name))>"Retirer"</button>
                                    </form>
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
                <p>"Sous-total : " <span data-cart-subtotal="">(cart.subtotal().unwrap_or_default())</span></p>
                if let (Some(discount), Some(amount)) = (&cart.discount, cart.discount_amount()) {
                    <p data-cart-discount=(&discount.code)>"Code promo " <code>(&discount.code)</code> " : " <span data-discount-amount="">(amount)</span></p>
                }
                if let Some(refused) = cart.discount_refused {
                    <p class="notice" role="alert" data-discount-refused=(refused)>(say(refused))</p>
                }
                <form class="discount" method="post" action=(href!(discount::discount))>
                    if let Some(code) = &cart.discount_code {
                        <span>"Code " <code>(code)</code></span>
                        <button name="remove" value="1">"Retirer le code"</button>
                    } else {
                        <label>
                            "Code promo "
                            <input type="text" name="code" maxlength="32" autocomplete="off">
                        </label>
                        <button>"Appliquer"</button>
                    }
                </form>
                <p>"Total : " <strong class="price" data-cart-total="">(cart.total().unwrap_or_default())</strong></p>
                if cart.can_check_out() {
                    <p><a href=(href!(crate::app::checkout::index))>"Commander"</a></p>
                }
            } else {
                <p data-cart-empty="">"Votre panier est vide."</p>
                <p><a href=(href!(crate::app::index))>"Continuer mes achats"</a></p>
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
    line: &CartLineView,
    currency: &str,
    back: &str,
    #[default] from: &str,
) -> Result<impl View> {
    let variant_id = line.variant_id.clone().unwrap_or_default();
    let name = if line.name.is_empty() {
        "Un produit qui n'est plus vendu".to_owned()
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
                    <a href=(href!(crate::app::p::slug::index, crate::app::p::slug::Slug(slug.clone())))>(&name)</a>
                } else {
                    (&name)
                }
                if let Some(label) = &line.variant_label { <small class="variant">(label)</small> }
                if let Some(offer) = &line.offer {
                    <small class="offer-note" data-line-offer=(&offer.offer_id)>"Offre : 1 à " (format_money(offer.unit_amount_cents, currency))</small>
                }
                if line.amount_cents.is_some() {
                    <span class="qty">
                        <form method="post" action=(href!(quantity::quantity))>
                            <input type="hidden" name="product_id" value=(&line.product_id)>
                            <input type="hidden" name="variant_id" value=(&variant_id)>
                            <input type="hidden" name="quantity" value=(line.quantity.saturating_sub(1))>
                            <input type="hidden" name="back" value=(back)>
                            <input type="hidden" name="from" value=(from)>
                            <button aria-label=(format!("Un de moins : {}", line.name))>"−"</button>
                        </form>
                        <span data-line-quantity="">(line.quantity)</span>
                        <form method="post" action=(href!(quantity::quantity))>
                            <input type="hidden" name="product_id" value=(&line.product_id)>
                            <input type="hidden" name="variant_id" value=(&variant_id)>
                            <input type="hidden" name="quantity" value=(line.quantity + 1)>
                            <input type="hidden" name="back" value=(back)>
                            <input type="hidden" name="from" value=(from)>
                            <button aria-label=(format!("Un de plus : {}", line.name)) disabled=(line.quantity >= 99)>"+"</button>
                        </form>
                    </span>
                } else {
                    <small class="off-sale">"Plus en vente : retirez-le pour commander le reste."</small>
                }
            </span>
            <span class="amount">
                if let Some(amount) = line.amount_cents {
                    (format_money(amount, currency))
                }
                <form method="post" action=(href!(remove::remove))>
                    <input type="hidden" name="product_id" value=(&line.product_id)>
                    <input type="hidden" name="variant_id" value=(&variant_id)>
                    <input type="hidden" name="back" value=(back)>
                    <input type="hidden" name="from" value=(from)>
                    <button class="remove" aria-label=(format!("Retirer {}", line.name))>"×"</button>
                </form>
            </span>
        </li>
    })
}
