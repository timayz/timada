//! `/cart`: "votre panier". Prices and names are read server-side when a
//! line is added; the form only says which product and how many.

use serde::Deserialize;
use timada_cart::{AddLine, CartDetailsView, CartError, CartLine};
use timada_catalog::load_product_page;
use timada_pricing::{load_product_price, price_id};
use timada_promotion::{
    CodeKind, discount_id, load_discount_details, load_voucher_balance, quote_code, voucher_id,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page, path_param as param},
    view::{View, component, view},
};

use super::{
    account,
    catalog::{self, ProductId, available_stock},
    checkout, document,
    format::money,
};
use crate::{
    Store,
    auth::{current_account, require_account},
    cart_session::{current_cart, ensure_cart, forget_cart, fresh_cart},
};

#[page("/cart")]
pub async fn show() -> Result<impl View> {
    Ok(view! { cart_view(error: None) })
}

#[derive(Debug, Deserialize)]
pub struct AddForm {
    product_id: String,
    quantity: u32,
}

/// Adds a product, or tops up the line when it is already in the cart.
#[page(POST "/cart/add")]
pub async fn add(cx: &Cx, Form(form): Form<AddForm>) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let outcome = add_to_cart(cx, store, &form).await?;
    match outcome {
        Ok(()) => Err(see_other(href!(show).resolve(cx)).into()),
        Err(message) => Ok(view! { cart_view(error: Some(message)) }),
    }
}

async fn add_to_cart(
    cx: &Cx,
    store: &Store,
    form: &AddForm,
) -> Result<std::result::Result<(), String>> {
    let product = load_product_page(&store.executor, &form.product_id)
        .await?
        .filter(|p| !p.archived);
    let price = load_product_price(&store.executor, price_id(&form.product_id))
        .await?
        .filter(|p| !p.withdrawn);
    let (Some(product), Some(price)) = (product, price) else {
        return Ok(Err(
            "Ce produit n'est plus disponible à la vente.".to_owned()
        ));
    };

    let in_cart = match current_cart(cx).await {
        Ok(cart) => cart
            .as_ref()
            .and_then(|c| c.lines.iter().find(|l| l.product_id == form.product_id))
            .map_or(0, |l| l.quantity),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let wanted = in_cart.saturating_add(form.quantity);
    let available = available_stock(store, &form.product_id).await?;
    if wanted > available {
        return Ok(Err(format!(
            "Seulement {available} exemplaire(s) disponible(s) pour « {} ».",
            product.name
        )));
    }

    let cart_id = ensure_cart(cx).await?;
    let command = timada_cart::Command(&store.executor);
    let result = if in_cart == 0 {
        command
            .add_line(
                &cart_id,
                AddLine {
                    product_id: form.product_id.clone(),
                    name: product.name,
                    quantity: form.quantity,
                    unit_price: price.price_incl_tax,
                    warranty_months: product.warranty_months,
                },
            )
            .await
    } else {
        command
            .change_line_quantity(&cart_id, form.product_id.clone(), wanted)
            .await
    };
    user_facing(result)
}

#[derive(Debug, Deserialize)]
pub struct QuantityForm {
    quantity: u32,
}

#[page(POST "/cart/lines/{product_id}/quantity")]
pub async fn change_quantity(cx: &Cx, Form(form): Form<QuantityForm>) -> Result<impl View> {
    let product_id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let available = available_stock(store, &product_id).await?;
    let outcome = if form.quantity > available {
        Err(format!(
            "Seulement {available} exemplaire(s) disponible(s)."
        ))
    } else {
        let cart_id = ensure_cart(cx).await?;
        user_facing(
            timada_cart::Command(&store.executor)
                .change_line_quantity(&cart_id, product_id, form.quantity)
                .await,
        )?
    };
    match outcome {
        Ok(()) => Err(see_other(href!(show).resolve(cx)).into()),
        Err(message) => Ok(view! { cart_view(error: Some(message)) }),
    }
}

#[page(POST "/cart/lines/{product_id}/remove")]
pub async fn remove(cx: &Cx) -> Result<impl View> {
    let product_id = param::<ProductId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    let cart_id = ensure_cart(cx).await?;
    let outcome = user_facing(
        timada_cart::Command(&store.executor)
            .remove_line(&cart_id, product_id)
            .await,
    )?;
    match outcome {
        Ok(()) => Err(see_other(href!(show).resolve(cx)).into()),
        Err(message) => Ok(view! { cart_view(error: Some(message)) }),
    }
}

#[derive(Debug, Deserialize)]
pub struct PromoForm {
    code: String,
}

/// Records a promo code or voucher. The cart context takes any code; the
/// storefront refuses the ones the promotion context would not honour.
#[page(POST "/cart/promo")]
pub async fn apply_promo(cx: &Cx, Form(form): Form<PromoForm>) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let outcome = match promo_problem(store, &form.code).await? {
        Some(problem) => Err(problem.to_owned()),
        None => {
            let cart_id = ensure_cart(cx).await?;
            user_facing(
                timada_cart::Command(&store.executor)
                    .apply_promo_code(&cart_id, form.code)
                    .await,
            )?
        }
    };
    match outcome {
        Ok(()) => Err(see_other(href!(show).resolve(cx)).into()),
        Err(message) => Ok(view! { cart_view(error: Some(message)) }),
    }
}

#[page(POST "/cart/promo/remove")]
pub async fn remove_promo(cx: &Cx) -> Result<impl View> {
    let store = app_context::<Store>(cx);
    let cart_id = ensure_cart(cx).await?;
    let outcome = user_facing(
        timada_cart::Command(&store.executor)
            .remove_promo_code(&cart_id)
            .await,
    )?;
    match outcome {
        Ok(()) => Err(see_other(href!(show).resolve(cx)).into()),
        Err(message) => Ok(view! { cart_view(error: Some(message)) }),
    }
}

#[derive(Debug, Deserialize)]
pub struct SaveForm {
    name: String,
}

/// Parks the cart in the shopper's saved carts; the browser starts a new one.
#[page(POST "/cart/save")]
pub async fn save(cx: &Cx, Form(form): Form<SaveForm>) -> Result<impl View> {
    let account = require_account(cx).await?;
    let store = app_context::<Store>(cx);
    let cart_id = ensure_cart(cx).await?;
    let carts = timada_cart::Command(&store.executor);
    let saved = match carts.assign_customer(&cart_id, &account.customer_id).await {
        Ok(()) => carts.save_cart(&cart_id, form.name).await,
        Err(err) => Err(err),
    };
    let error = match saved {
        Ok(()) => {
            forget_cart(cx);
            return Err(see_other(href!(account::saved_carts).resolve(cx)).into());
        }
        Err(CartError::Required(_)) => "Donnez un nom à ce panier.",
        Err(CartError::EmptyCart) => "Votre panier est vide.",
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Ok(view! { cart_view(error: Some(error.to_owned())) })
}

/// Why a code cannot be used right now, if it cannot.
async fn promo_problem(store: &Store, code: &str) -> anyhow::Result<Option<&'static str>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    if let Some(discount) = load_discount_details(&store.executor, discount_id(code)).await? {
        let exhausted = discount
            .max_redemptions
            .is_some_and(|max| discount.redeemed >= max);
        return Ok(if !discount.active {
            Some("Ce code promo n'est plus actif.")
        } else if discount.valid_until.is_some_and(|until| until <= now) {
            Some("Ce code promo a expiré.")
        } else if exhausted {
            Some("Ce code promo a atteint son nombre maximal d'utilisations.")
        } else {
            None
        });
    }
    if let Some(voucher) = load_voucher_balance(&store.executor, voucher_id(code)).await? {
        return Ok(if voucher.cancelled {
            Some("Ce bon d'achat a été annulé.")
        } else if voucher.expires_at.is_some_and(|at| at <= now) {
            Some("Ce bon d'achat a expiré.")
        } else if !voucher.remaining.is_positive() {
            Some("Ce bon d'achat est épuisé.")
        } else {
            None
        });
    }
    Ok(Some("Code promo ou bon d'achat inconnu."))
}

/// The cart's code as the cart and checkout pages show it.
pub(super) struct PromoLine {
    pub label: String,
    /// What the code takes off and the subtotal once it did; `None` when the
    /// code stopped being usable since it was typed in.
    pub effect: Option<(String, String)>,
}

/// An estimate on the goods alone — the binding amount is set when the order
/// is placed, by the order context redeeming the code.
pub(super) async fn promo_line(
    store: &Store,
    cart: &CartDetailsView,
) -> anyhow::Result<Option<PromoLine>> {
    promo_line_on(store, cart.promo_code.as_ref(), &cart.subtotal).await
}

/// [`promo_line`] on a subtotal other than the cart's own — the checkout
/// prices the goods for the delivery zone first.
pub(super) async fn promo_line_on(
    store: &Store,
    code: Option<&String>,
    subtotal: &timada_core::Money,
) -> anyhow::Result<Option<PromoLine>> {
    let Some(code) = code else {
        return Ok(None);
    };
    let quote = quote_code(&store.executor, code, subtotal, subtotal).await?;
    let Some(quote) = quote else {
        return Ok(Some(PromoLine {
            label: code.clone(),
            effect: None,
        }));
    };
    let label = match quote.kind {
        CodeKind::Discount => format!("Code promo {}", quote.code),
        CodeKind::Voucher => format!("Bon d'achat {}", quote.code),
    };
    let net = subtotal.checked_sub(&quote.amount)?;
    Ok(Some(PromoLine {
        label,
        effect: Some((money(&quote.amount), money(&net))),
    }))
}

/// Domain refusals become a message on the cart page; the rest is a 500.
fn user_facing(
    result: std::result::Result<(), CartError>,
) -> Result<std::result::Result<(), String>> {
    match result {
        Ok(()) => Ok(Ok(())),
        Err(CartError::InvalidQuantity) => Ok(Err("La quantité doit être d'au moins 1.".into())),
        Err(CartError::LineNotFound(_)) => Ok(Err("Ce produit n'est plus dans le panier.".into())),
        Err(CartError::Required(_)) => Ok(Err("Saisissez un code.".into())),
        Err(CartError::Money(_)) => Ok(Err(
            "Ce produit n'est pas vendu dans la devise du panier.".into()
        )),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

#[component]
async fn cart_view(cx: &Cx, error: Option<String>) -> Result<impl View> {
    let (cart, price_notices) = match fresh_cart(cx).await? {
        Some((cart, notices)) => (Some(cart).filter(|c| !c.lines.is_empty()), notices),
        None => (None, Vec::new()),
    };
    let promo = match &cart {
        Some(cart) => promo_line(app_context::<Store>(cx), cart).await?,
        None => None,
    };
    let signed_in = match current_account(cx).await {
        Ok(account) => account.is_some(),
        Err(err) => return Err(anyhow::anyhow!("{err:#}").into()),
    };
    let login_link = href!(account::login)
        .query([("next", href!(show).resolve(cx))])
        .resolve(cx);

    Ok(view! {
        document(
            title: "Votre panier",
            <h1>"Votre panier"</h1>
            if let Some(error) = &error { <p role="alert" class="error">(error.clone())</p> }
            for notice in &price_notices { <p role="status" class="notice">(notice.clone())</p> }
            match &cart {
                Some(cart) => {
                    <table>
                        <caption class="muted">"Articles dans votre panier"</caption>
                        <thead>
                            <tr>
                                <th scope="col">"Produit"</th>
                                <th scope="col" class="num">"Prix unitaire"</th>
                                <th scope="col">"Quantité"</th>
                                <th scope="col" class="num">"Total"</th>
                                <th scope="col"><span class="muted">"Actions"</span></th>
                            </tr>
                        </thead>
                        <tbody>
                            for line in &cart.lines { cart_row(line: line) }
                        </tbody>
                    </table>
                    <table class="totals">
                        <tbody>
                            promo_totals(subtotal: money(&cart.subtotal), promo: &promo)
                        </tbody>
                    </table>
                    <form method="post" action=(href!(apply_promo))>
                        <label for="code">"Code promo ou bon d'achat"</label>
                        " "
                        <input id="code" name="code" required=(true) autocomplete="off" value=(cart.promo_code.clone().unwrap_or_default())>
                        " "
                        <button type="submit">"Appliquer"</button>
                    </form>
                    if let Some(code) = &cart.promo_code {
                        <form method="post" action=(href!(remove_promo)) class="inline">
                            <button type="submit" class="link">"Retirer le code " <span class="muted">(code.clone())</span></button>
                        </form>
                    }
                    promo_notice(promo: &promo)
                    if signed_in {
                        <form method="post" action=(href!(save))>
                            <label for="cart-name">"Sauvegarder ce panier pour plus tard"</label>
                            " "
                            <input id="cart-name" name="name" required=(true) maxlength="80" placeholder="Nom du panier" autocomplete="off">
                            " "
                            <button type="submit">"Sauvegarder"</button>
                        </form>
                    } else {
                        <p class="muted"><a href=(login_link.clone())>"Connectez-vous"</a> " pour sauvegarder ce panier."</p>
                    }
                    <p>
                        <a href=(href!(checkout::show))><strong>"Passer commande"</strong></a>
                        " · "
                        <a href=(href!(catalog::home))>"Continuer mes achats"</a>
                    </p>
                }
                None => {
                    <p class="muted">"Votre panier est vide."</p>
                    <p><a href=(href!(catalog::home))>"Voir le catalogue"</a></p>
                }
            }
        )
    })
}

/// The subtotal rows of the cart and checkout pages, net of the cart's code.
#[component]
pub(super) async fn promo_totals(subtotal: String, promo: &Option<PromoLine>) -> Result<impl View> {
    let effect = promo
        .as_ref()
        .and_then(|p| Some((p.label.clone(), p.effect.clone()?)));
    Ok(view! {
        match &effect {
            Some((label, (amount, net))) => {
                <tr><td>"Sous-total TTC"</td><td class="num">(subtotal.clone())</td></tr>
                <tr><td>(label.clone())</td><td class="num">"− " (amount.clone())</td></tr>
                <tr class="total"><td>"Sous-total après remise"</td><td class="num">(net.clone())</td></tr>
            }
            None => {
                <tr class="total"><td>"Sous-total TTC"</td><td class="num">(subtotal.clone())</td></tr>
            }
        }
    })
}

/// Says so when the cart's code will not be honoured after all.
#[component]
pub(super) async fn promo_notice(promo: &Option<PromoLine>) -> Result<impl View> {
    let dead_code = promo
        .as_ref()
        .filter(|p| p.effect.is_none())
        .map(|p| p.label.clone());
    Ok(view! {
        if let Some(code) = &dead_code {
            <p role="status" class="notice">"Le code " <strong>(code.clone())</strong> " n'est plus valable : il ne sera pas appliqué à la commande."</p>
        }
    })
}

#[component]
async fn cart_row(cx: &Cx, line: &CartLine) -> Result<impl View> {
    let product = href!(catalog::product_page, ProductId(line.product_id.clone())).resolve(cx);
    let quantity_action = href!(change_quantity, ProductId(line.product_id.clone())).resolve(cx);
    let remove_action = href!(remove, ProductId(line.product_id.clone())).resolve(cx);
    let field = format!("quantity-{}", line.product_id);
    let total = line.unit_price.checked_mul(line.quantity)?;

    Ok(view! {
        <tr>
            <th scope="row">
                <a href=(product)>(line.name.clone())</a>
                <br>
                <span class="muted">"Garantie " (line.warranty_months.to_string()) " mois"</span>
            </th>
            <td class="num">(money(&line.unit_price))</td>
            <td>
                <form method="post" action=(quantity_action) class="inline">
                    <label for=(field.clone()) class="muted">"Quantité de " (line.name.clone())</label>
                    <input id=(field) name="quantity" type="number" min="1" value=(line.quantity.to_string()) required=(true)>
                    " "
                    <button type="submit">"Mettre à jour"</button>
                </form>
            </td>
            <td class="num">(money(&total))</td>
            <td>
                <form method="post" action=(remove_action) class="inline">
                    <button type="submit" class="link">"Supprimer " <span class="muted">(line.name.clone())</span></button>
                </form>
            </td>
        </tr>
    })
}
