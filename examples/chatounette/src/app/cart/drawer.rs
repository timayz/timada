//! The cart drawer: the cart sliding in from the right on every page, with
//! what goes well with it beside. Without the page's script it opens on the
//! `#cart` fragment the cart forms come back to; with it, nothing reloads
//! (the library's cart script).

use timada_storefront::{
    Back, add_to_cart_form, cart_drawer_root, discount_form, drawer_express, drawer_pay_error,
    pages::{DrawerOptions, cart_drawer as drawer_model},
    price, product_image,
};
use topcoat::{
    Result,
    context::Cx,
    router::href,
    view::{View, attributes, view},
};

use super::summary_line;

/// The cart drawer, closed until the `#cart` fragment or the page's script
/// opens it.
#[topcoat::view::component]
pub async fn cart_drawer(cx: &Cx) -> Result<impl View> {
    let drawer = drawer_model(cx, &DrawerOptions::default()).await?;
    let from = drawer.from.clone();

    Ok(view! {
        cart_drawer_root(drawer: &drawer, attrs: attributes! { class="drawer" },
            <a class="drawer-backdrop" href="#" tabindex="-1" aria-hidden="true" data-drawer-close=""></a>
            <div class="drawer-sheet">
                <section class="drawer-panel" role="dialog" aria-labelledby="cart-drawer-title">
                    <header class="drawer-head">
                        <h2 id="cart-drawer-title">"Votre panier (" (drawer.count) ")"</h2>
                        <a class="drawer-close" href="#" aria-label="Fermer le panier" data-drawer-close="">"×"</a>
                    </header>
                    if let Some(error) = &drawer.error {
                        <p class="notice" role="alert" data-cart-error=(&error.code)>(&error.text)</p>
                    }
                    if let Some(cart) = &drawer.cart {
                        <div class="summary">
                            <ul class="lines">
                                for item in &drawer.lines {
                                    summary_line(item: item, back: Back::Drawer, from: Some(from.as_str()))
                                }
                            </ul>
                            <details class="promo" open=(drawer.promo_open)>
                                <summary>"Vous avez un code promo ?"</summary>
                                discount_form(back: Back::Drawer, from: Some(from.as_str()), attrs: attributes! { class="discount" },
                                    if let Some(code) = &cart.discount_code {
                                        <span>"Code " <code>(code)</code></span>
                                        <button name="remove" value="1">"Retirer le code"</button>
                                    } else {
                                        <label>
                                            <span class="vh">"Code promo"</span>
                                            <input type="text" name="code" maxlength="32" autocomplete="off" placeholder="Code promo">
                                        </label>
                                        <button>"Appliquer"</button>
                                    }
                                )
                                if let Some(refused) = &drawer.discount_refused {
                                    <p class="notice" data-discount-refused=(&refused.code)>(&refused.text)</p>
                                }
                            </details>
                            <dl class="totals">
                                <dt>"Sous-total"</dt>
                                <dd data-drawer-subtotal="">(&drawer.subtotal)</dd>
                                if let (Some(discount), Some(amount)) = (&cart.discount, &drawer.discount_amount) {
                                    <dt>"Code promo " <code>(&discount.code)</code></dt>
                                    <dd data-drawer-discount=(&discount.code)>(amount)</dd>
                                }
                                <dt class="total">"Total"</dt>
                                <dd class="total"><strong class="price" data-drawer-total="">(&drawer.total)</strong></dd>
                            </dl>
                            <p class="hint">"Taxes et frais de livraison calculés à l'étape de paiement."</p>
                        </div>
                        if drawer.has_payments {
                            drawer_express(attrs: attributes! { class="drawer-express" },
                                drawer_pay_error(attrs: attributes! { class="notice" })
                            )
                        }
                        if drawer.can_check_out {
                            <a class="place" href=(&drawer.checkout_url)>"Commander · " (&drawer.total)</a>
                        }
                        <a class="drawer-continue" href="#" data-drawer-close="">"Continuer mes achats"</a>
                    } else {
                        <p class="drawer-empty" data-cart-empty="">"Votre panier est vide."</p>
                        <a class="drawer-continue" href=(&drawer.home_url)>"Continuer mes achats"</a>
                    }
                </section>
                if !drawer.offered.is_empty() {
                    <section class="upsell" aria-labelledby="cart-upsell-title" data-upsell="">
                        <h2 id="cart-upsell-title">"Vous aimerez aussi"</h2>
                        <ul>
                            for product in &drawer.offered {
                                <li data-offer=(&product.slug)>
                                    <a href=(href!(crate::app::p::slug::index, crate::app::p::slug::Slug(product.slug.clone())))>
                                        if let Some(image) = product.image() {
                                            product_image(image: image, attrs: attributes! { class="offer-pic" })
                                        }
                                        <span>(&product.name)</span>
                                    </a>
                                    price(product: product, attrs: attributes! { class="price" })
                                    if product.options.is_empty() {
                                        add_to_cart_form(product: product, quantity: Some(1), from: Some(from.as_str()),
                                            <button aria-label=(format!("Ajouter {}", product.name))>"Ajouter"</button>
                                        )
                                    } else {
                                        <a class="offer-options" href=(href!(crate::app::p::slug::index, crate::app::p::slug::Slug(product.slug.clone())))
                                            aria-label=(format!("Choisir les options de {}", product.name))>"Choisir les options"</a>
                                    }
                                </li>
                            }
                        </ul>
                    </section>
                }
            </div>
        )
    })
}

/// The drawer's look, in the shop's coral and ink. It shows on the `#cart`
/// fragment (no script: the page loads with it open, no slide) or slides in
/// on the `open` class (the script, which adds `shut` to slide it out while
/// the fragment is still there).
pub const DRAWER_CSS: &str = r#"
.cart-link { position: relative; text-decoration: none; color: var(--ink); font-weight: 600; }
.cart-link .count { display: inline-block; min-width: 1.3rem; height: 1.3rem; line-height: 1.3rem; margin-left: .3rem; border-radius: 1rem; background: var(--coral-dark); color: #fff; font-size: .75rem; text-align: center; }
.drawer { position: fixed; inset: 0; z-index: 50; display: flex; justify-content: flex-end; visibility: hidden; pointer-events: none; }
.drawer.open, .drawer:target:not(.shut) { visibility: visible; pointer-events: auto; }
.drawer.shut { transition: visibility 0s linear .3s; }
.drawer-backdrop { position: absolute; inset: 0; background: rgba(61,59,92,.4); opacity: 0; }
.drawer.open .drawer-backdrop, .drawer:target:not(.shut) .drawer-backdrop { opacity: 1; }
.drawer-sheet { position: relative; display: flex; height: 100%; max-width: 100%; transform: translateX(100%); box-shadow: -8px 0 24px rgba(0,0,0,.12); }
.drawer.open .drawer-sheet, .drawer:target:not(.shut) .drawer-sheet { transform: none; }
.drawer.open .drawer-sheet, .drawer.shut .drawer-sheet { transition: transform .3s ease; }
.drawer.open .drawer-backdrop, .drawer.shut .drawer-backdrop { transition: opacity .3s; }
.drawer-panel { order: 2; width: 26rem; max-width: 100vw; box-sizing: border-box; height: 100%; overflow-y: auto; background: #fff; padding: 1rem 1.25rem 1.5rem; display: flex; flex-direction: column; gap: .75rem; }
.drawer-head { display: flex; justify-content: space-between; align-items: center; margin: 0; border-bottom: 1px solid var(--line); padding-bottom: .75rem; }
.drawer-head h2 { margin: 0; font-size: 1rem; font-weight: 600; color: var(--ink); }
.drawer-close { font-size: 1.6rem; line-height: 1; text-decoration: none; color: var(--ink); padding: 0 .25rem; }
.drawer .summary ul.lines li { border-bottom: 1px solid var(--line); }
.drawer .summary .what a { color: var(--ink); text-decoration: none; }
.drawer .summary .what small { display: block; color: var(--coral-dark); font-size: .8rem; }
.drawer .summary .qty button { cursor: pointer; }
.drawer .summary .qty button:disabled { cursor: default; opacity: .4; }
.drawer .summary details.promo { margin: .75rem 0; }
.drawer .hint { margin: .5rem 0 0; font-size: .8rem; }
.drawer a.place { display: block; text-align: center; text-decoration: none; padding: .9rem; border-radius: 2rem; background: var(--coral); color: #fff; font-weight: 600; letter-spacing: .03em; text-transform: uppercase; box-shadow: 0 6px 16px rgba(242,139,143,.35); }
.drawer a.place:hover { background: var(--coral-dark); }
.drawer-continue { display: block; text-align: center; color: var(--ink); font-size: .9rem; }
.drawer-empty { color: var(--muted); text-align: center; margin: 2rem 0 1rem; }
.drawer-express { margin: 0; }
.upsell { order: 1; width: 11rem; height: 100%; box-sizing: border-box; overflow-y: auto; background: var(--coral-wash); padding: 1rem .75rem; border-right: 1px solid var(--line); }
.upsell h2 { font-size: .85rem; font-weight: 600; color: var(--ink); text-align: center; margin: 0 0 1rem; }
.upsell ul { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: 1.25rem; }
.upsell li { text-align: center; font-size: .8rem; color: var(--ink); }
.upsell li > a { display: block; color: inherit; text-decoration: none; }
.upsell .offer-pic { display: block; width: 4.5rem; height: 4.5rem; object-fit: cover; margin: 0 auto .4rem; border-radius: .4rem; background: #fff; }
.upsell .price { display: block; margin: .2rem 0 .4rem; font-weight: 400; color: var(--muted); }
.upsell form { margin: 0; }
.upsell button, .upsell .offer-options { border: 1px solid var(--coral); background: #fff; color: var(--coral-dark); border-radius: .3rem; padding: .25rem .9rem; font-size: .75rem; cursor: pointer; text-decoration: none; display: inline-block; }
.upsell button:hover, .upsell .offer-options:hover { background: var(--coral); color: #fff; }
@media (max-width: 899px) {
  .drawer-sheet { flex-direction: column; width: 26rem; }
  .drawer-panel { order: 1; flex: 1; width: 100%; }
  .upsell { order: 2; width: 100%; height: auto; flex: none; border-right: 0; border-top: 1px solid var(--line); padding: .75rem; }
  .upsell h2 { text-align: left; margin-bottom: .5rem; }
  .upsell ul { flex-direction: row; overflow-x: auto; gap: 1rem; scroll-snap-type: x mandatory; }
  .upsell li { flex: 0 0 7rem; scroll-snap-align: start; }
}
@media (prefers-reduced-motion: reduce) {
  .drawer, .drawer-backdrop, .drawer-sheet { transition: none; }
}
"#;
