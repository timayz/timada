//! `/cart`: what this browser is about to buy, priced from the store as it
//! stands, with the forms that change it.

pub mod drawer;

use timada_storefront::{
    Back, cart_quantity_form, cart_remove_form, discount_form,
    pages::{LineModel, cart_page},
};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, view},
};

use crate::app::document;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = cart_page(cx).await?;

    Ok(view! {
        document(
            title: "Votre panier",
            no_drawer: true,
            <h1>"Votre panier"</h1>
            if let Some(error) = &page.error {
                <p class="notice" role="alert">(&error.text)</p>
            }
            if let Some(cart) = &page.cart {
                <table class="cart" data-cart="">
                    <thead>
                        <tr><th></th><th>"Produit"</th><th>"Prix"</th><th>"Quantité"</th><th>"Montant"</th><th></th></tr>
                    </thead>
                    <tbody>
                        for item in &page.lines {
                            <tr data-cart-line=(&item.line.name) data-available=(if item.available { "true" } else { "false" })>
                                <td>
                                    if let Some(url) = &item.line.image_url {
                                        <img src=(url) alt="" loading="lazy">
                                    }
                                </td>
                                <td>
                                    if let Some(url) = &item.url {
                                        <a href=(url)>(&item.name)</a>
                                    } else {
                                        (&item.name)
                                    }
                                    if let Some(label) = &item.line.variant_label {
                                        <div><small>(label)</small></div>
                                    }
                                    if let (Some(offer), Some(unit)) = (&item.line.offer, &item.offer_unit) {
                                        <div><small class="offer-note" data-line-offer=(&offer.offer_id)>"Offre : 1 à " (unit)</small></div>
                                    }
                                    if !item.available {
                                        <div><small>"Plus en vente : retirez-le pour commander le reste."</small></div>
                                    }
                                </td>
                                <td>
                                    if let Some(unit) = &item.unit { (unit) }
                                </td>
                                <td>
                                    cart_quantity_form(line: &item.line, back: Back::Cart,
                                        <label>
                                            <span hidden="">"Quantité de " (&item.name)</span>
                                            <input type="number" name="quantity" min="0" max="99" value=(item.line.quantity) aria-label=(format!("Quantité de {}", item.name))>
                                        </label>
                                        <button>"Mettre à jour"</button>
                                    )
                                </td>
                                <td data-line-amount="">
                                    if let Some(amount) = &item.amount { (amount) }
                                </td>
                                <td>
                                    cart_remove_form(line: &item.line, back: Back::Cart,
                                        <button aria-label=(format!("Retirer {}", item.name))>"Retirer"</button>
                                    )
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
                <p>"Sous-total : " <span data-cart-subtotal="">(page.subtotal.as_deref().unwrap_or_default())</span></p>
                if let (Some(discount), Some(amount)) = (&cart.discount, &page.discount_amount) {
                    <p data-cart-discount=(&discount.code)>"Code promo " <code>(&discount.code)</code> " : " <span data-discount-amount="">(amount)</span></p>
                }
                if let Some(refused) = &page.discount_refused {
                    <p class="notice" role="alert" data-discount-refused=(&refused.code)>(&refused.text)</p>
                }
                discount_form(back: Back::Cart, attrs: topcoat::view::attributes! { class="discount" },
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
                )
                <p>"Total : " <strong class="price" data-cart-total="">(page.total.as_deref().unwrap_or_default())</strong></p>
                if page.can_check_out {
                    <p><a href=(&page.checkout_url)>"Commander"</a></p>
                }
            } else {
                <p data-cart-empty="">"Votre panier est vide."</p>
                <p><a href=(&page.home_url)>"Continuer mes achats"</a></p>
            }
        )
    })
}

/// One line of a cart beside the checkout or in the drawer: the picture,
/// the name, a −/+ stepper and the amount with a remove button. Each button
/// is its own small form, so it works without the page's script; `back` and
/// `from` say where it comes back to.
#[topcoat::view::component]
pub async fn summary_line(
    item: &LineModel,
    back: Back,
    #[default] from: Option<&str>,
) -> Result<impl View> {
    let line = &item.line;
    Ok(view! {
        <li data-summary-line=(&line.name) data-available=(if item.available { "true" } else { "false" })>
            <span class="pic">
                if let Some(url) = &line.image_url {
                    <img src=(url) alt="" width="56" height="56" loading="lazy">
                }
                <span class="badge" aria-hidden="true">(line.quantity)</span>
            </span>
            <span class="what">
                if let Some(url) = &item.url {
                    <a href=(url)>(&item.name)</a>
                } else {
                    (&item.name)
                }
                if let Some(label) = &line.variant_label { <small class="variant">(label)</small> }
                if let (Some(offer), Some(unit)) = (&line.offer, &item.offer_unit) {
                    <small class="offer-note" data-line-offer=(&offer.offer_id)>"Offre : 1 à " (unit)</small>
                }
                if item.available {
                    <span class="qty">
                        cart_quantity_form(line: line, quantity: Some(line.quantity.saturating_sub(1)), back: back, from: from,
                            <button aria-label=(format!("Un de moins : {}", line.name))>"−"</button>
                        )
                        <span data-line-quantity="">(line.quantity)</span>
                        cart_quantity_form(line: line, quantity: Some(line.quantity + 1), back: back, from: from,
                            <button aria-label=(format!("Un de plus : {}", line.name)) disabled=(line.quantity >= 99)>"+"</button>
                        )
                    </span>
                } else {
                    <small class="off-sale">"Plus en vente : retirez-le pour commander le reste."</small>
                }
            </span>
            <span class="amount">
                if let Some(amount) = &item.amount { (amount) }
                cart_remove_form(line: line, back: back, from: from,
                    <button class="remove" aria-label=(format!("Retirer {}", line.name))>"×"</button>
                )
            </span>
        </li>
    })
}
