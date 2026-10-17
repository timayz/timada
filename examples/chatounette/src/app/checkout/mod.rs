//! `/checkout`: one page from the cart to the payment. The shopper's
//! details, the address and Stripe's Payment Element sit side by side with
//! the cart, which can still be changed there. The library's checkout
//! script shows Stripe's form before any payment is open, asks the store to
//! pay on submit and confirms with Stripe, which comes back to
//! `/checkout/complete`. The merchant's checkout offers show as cards above
//! the payment, each with an Add button that puts its item in the cart and
//! comes back here.

pub mod complete;

use timada_storefront::{
    Back, billing_fieldset, billing_toggle, checkout_form, checkout_script, country_select,
    discount_form, express_mount, express_section, offer_form,
    pages::{CheckoutOptions, checkout_page, initials},
    payment_element, payment_error, place_button, save_address_checkbox, shipping_method_input,
};
use topcoat::{
    Result,
    context::Cx,
    router::page,
    view::{View, attributes, view},
};

use crate::app::cart::summary_line;
use crate::app::document;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = checkout_page(cx, &CheckoutOptions::default()).await?;

    Ok(view! {
        document(
            title: "Commander",
            no_drawer: true,
            <ol class="steps" aria-label="Étapes de la commande">
                <li class="done"><a href=(&page.cart_url)>"Panier"</a></li>
                <li aria-current="step">"Livraison & paiement"</li>
                <li>"Confirmation"</li>
            </ol>
            <h1 class="vh">"Commander"</h1>
            if let Some(error) = &page.error {
                <p class="notice" role="alert">(&error.text)</p>
            }
            <div class="checkout-grid">
                <aside class="checkout-aside" aria-label="Récapitulatif de la commande">
                    <section class="card summary" data-checkout-summary="">
                        <h2>"Votre panier"</h2>
                        <ul class="lines">
                            for item in &page.lines {
                                summary_line(item: item, back: Back::Checkout)
                            }
                        </ul>
                        <details class="promo" open=(page.cart.discount_code.is_some())>
                            <summary>"Vous avez un code promo ? Saisissez-le ici"</summary>
                            discount_form(back: Back::Checkout, attrs: attributes! { class="discount" },
                                if let Some(code) = &page.cart.discount_code {
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
                            if let Some(refused) = &page.discount_refused {
                                <p class="notice" data-discount-refused=(&refused.code)>(&refused.text)</p>
                            }
                        </details>
                        <dl class="totals">
                            <dt>"Sous-total"</dt>
                            <dd data-checkout-subtotal="">(&page.subtotal)</dd>
                            if let (Some(discount), Some(amount)) = (&page.cart.discount, &page.discount_amount) {
                                <dt>"Code promo " <code>(&discount.code)</code></dt>
                                <dd data-checkout-discount=(&discount.code)>(amount)</dd>
                            }
                            <dt>"Livraison"</dt>
                            <dd data-checkout-shipping="">(&page.shipping_text)</dd>
                            <dt class="total">"Total"</dt>
                            <dd class="total">
                                <strong class="price" data-checkout-total="">(&page.total)</strong>
                                <small data-checkout-vat="" hidden=(page.vat_text.is_none())>
                                    if let Some(vat) = &page.vat_text { "(" (vat) ")" }
                                </small>
                            </dd>
                        </dl>
                    </section>
                    <section class="reassure">
                        <h2>"Commandez en toute confiance"</h2>
                        <ul>
                            <li>"Livraison gratuite dès 65€ d'achats"</li>
                            <li>"Livraison avec suivi en France, Belgique, Suisse"</li>
                            <li>"14 jours satisfait ou remboursé"</li>
                            <li>"Service client disponible 6/7J"</li>
                            <li>"Paiement sécurisé"</li>
                        </ul>
                    </section>
                    if !page.reviews.is_empty() {
                        <section class="reviews-aside" data-checkout-reviews="">
                            <h2>"Ce que disent nos clients"</h2>
                            for review in &page.reviews {
                                <figure class="review">
                                    <figcaption>
                                        <span class="avatar" aria-hidden="true">(initials(&review.author_name))</span>
                                        <span>(&review.author_name)</span>
                                    </figcaption>
                                    <div class="rating" aria-label=(format!("{} sur 5", review.rating))>(review.stars())</div>
                                    if let Some(title) = &review.title { <strong>(title)</strong> }
                                    <blockquote>(&review.body)</blockquote>
                                </figure>
                            }
                        </section>
                    }
                </aside>

                <div class="checkout-main">
                    if let Some(shopper) = &page.shopper {
                        <p class="returning" data-signed-in="">"Connecté en tant que " <strong>(&shopper.email)</strong></p>
                    } else if page.accounts_enabled {
                        <p class="returning" data-returning="">
                            "Déjà client ? " <a href=(&page.sign_in_url)>"Cliquez ici pour vous connecter"</a>
                        </p>
                    }
                    if page.setup.is_some() {
                        express_section(attrs: attributes! { class="express" aria-label="Paiement express" },
                            <h2>"Paiement express"</h2>
                            express_mount()
                            <p class="or"><span>"ou"</span></p>
                        )
                    }
                    checkout_form(page: &page, attrs: attributes! { class="checkout" },
                        <section class="card">
                            <h2>"Vos coordonnées"</h2>
                            <label for="checkout-email">"E-mail"</label>
                            <input id="checkout-email" name="email" type="email" required="" autocomplete="email" value=(&page.draft.email) readonly=(page.shopper.is_some())>
                            <label for="checkout-name">"Nom complet"</label>
                            <input id="checkout-name" name="name" required="" autocomplete="name" value=(&page.draft.name)>
                            <label for="checkout-phone">"Téléphone (facultatif)"</label>
                            <input id="checkout-phone" name="phone" type="tel" autocomplete="tel" value=(&page.draft.phone)>
                        </section>
                        <section class="card">
                            <h2>"Adresse de livraison"</h2>
                            if let Some(shopper) = page.shopper.as_ref().filter(|shopper| !shopper.addresses.is_empty()) {
                                <ul class="saved" data-saved-addresses="">
                                    for saved in &shopper.addresses {
                                        <li data-saved-address=(&saved.address.city) data-current=(page.shipping_to.as_deref() == Some(saved.id.as_str()))
                                            data-billing-current=(page.billing_to.as_deref() == Some(saved.id.as_str()))>
                                            (&saved.address.name) ", " (&saved.address.line1) ", "
                                            (&saved.address.postal_code) " " (&saved.address.city) " "
                                            <a href=(page.ship_here_url(cx, &saved.id))>"Livrer ici"</a> " "
                                            <a href=(page.bill_here_url(cx, &saved.id)) data-bill-here="">"Facturer ici"</a>
                                        </li>
                                    }
                                </ul>
                            }
                            <label for="checkout-line1">"Adresse"</label>
                            <input id="checkout-line1" name="line1" required="" autocomplete="address-line1" value=(&page.draft.line1)>
                            <details class="more" open=(!page.draft.line2.is_empty())>
                                <summary>"+ Ajouter une société, un appartement, un étage…"</summary>
                                <label for="checkout-line2">"Complément d'adresse (facultatif)"</label>
                                <input id="checkout-line2" name="line2" autocomplete="address-line2" value=(&page.draft.line2)>
                            </details>
                            <div class="row">
                                <span>
                                    <label for="checkout-city">"Ville"</label>
                                    <input id="checkout-city" name="city" required="" autocomplete="address-level2" value=(&page.draft.city)>
                                </span>
                                <span>
                                    <label for="checkout-postal-code">"Code postal"</label>
                                    <input id="checkout-postal-code" name="postal_code" required="" autocomplete="postal-code" value=(&page.draft.postal_code)>
                                </span>
                                <span>
                                    <label for="checkout-country">"Pays"</label>
                                    country_select(page: &page, attrs: attributes! { id="checkout-country" })
                                </span>
                            </div>
                            if page.asks_shipping {
                                <fieldset class="shipping-methods" data-shipping-methods="">
                                    <legend>"Mode de livraison"</legend>
                                    for choice in &page.shipping_choices {
                                        <label class="method" hidden=(!choice.ships_here)>
                                            shipping_method_input(choice: choice)
                                            <span class="name">(&choice.option.name)</span>
                                            <span class="price">(&choice.price)</span>
                                        </label>
                                    }
                                    <p class="notice" data-no-shipping="" hidden=(page.shipping.is_some())>"Cette boutique ne livre pas dans ce pays."</p>
                                </fieldset>
                            }
                            if page.business {
                                <label for="checkout-vat">"Numéro de TVA intracommunautaire (facultatif, pour une facture hors TVA)"</label>
                                <input id="checkout-vat" name="vat_number" autocomplete="off" placeholder="DE123456789" value=(&page.draft.vat_number)>
                            }
                            <label class="check">
                                billing_toggle(page: &page)
                                " Utiliser une autre adresse de facturation (facultatif)"
                            </label>
                            billing_fieldset(page: &page, attrs: attributes! { class="billing" },
                                <legend class="vh">"Adresse de facturation"</legend>
                                <label for="billing-name">"Nom ou société à facturer"</label>
                                <input id="billing-name" name="billing_name" required="" autocomplete="billing name" value=(&page.draft.billing_name)>
                                <label for="billing-line1">"Adresse (facturation)"</label>
                                <input id="billing-line1" name="billing_line1" required="" autocomplete="billing address-line1" value=(&page.draft.billing_line1)>
                                <label for="billing-line2">"Complément d'adresse de facturation (facultatif)"</label>
                                <input id="billing-line2" name="billing_line2" autocomplete="billing address-line2" value=(&page.draft.billing_line2)>
                                <div class="row">
                                    <span>
                                        <label for="billing-city">"Ville de facturation"</label>
                                        <input id="billing-city" name="billing_city" required="" autocomplete="billing address-level2" value=(&page.draft.billing_city)>
                                    </span>
                                    <span>
                                        <label for="billing-postal-code">"Code postal de facturation"</label>
                                        <input id="billing-postal-code" name="billing_postal_code" required="" autocomplete="billing postal-code" value=(&page.draft.billing_postal_code)>
                                    </span>
                                    <span>
                                        <label for="billing-country">"Pays de facturation"</label>
                                        country_select(page: &page, billing: true, attrs: attributes! { id="billing-country" })
                                    </span>
                                </div>
                                if page.shopper.is_some() {
                                    <label class="check">
                                        save_address_checkbox(billing: true)
                                        " Enregistrer cette adresse de facturation dans mon compte"
                                    </label>
                                }
                            )
                            <p class="hint" data-import-notice="">"Livraison hors UE ? Des droits de douane et taxes peuvent être dus à la réception."</p>
                            if page.shopper.is_some() {
                                <label class="check">
                                    save_address_checkbox(checked: true)
                                    " Enregistrer cette adresse dans mon compte"
                                </label>
                            }
                            <label for="checkout-note">"Remarque pour la boutique (facultatif)"</label>
                            <textarea id="checkout-note" name="note" rows="2">(&page.draft.note)</textarea>
                        </section>
                        if !page.offers.is_empty() {
                            <section class="card offers" aria-labelledby="checkout-offers-title" data-checkout-offers="">
                                <h2 id="checkout-offers-title">"Offre exclusive"</h2>
                                <ul>
                                    for item in &page.offers {
                                        <li data-checkout-offer=(&item.offer.name)>
                                            if let Some(url) = &item.offer.image_url {
                                                <img src=(url) alt="" width="56" height="56" loading="lazy">
                                            } else {
                                                <span></span>
                                            }
                                            <span class="what">
                                                <strong>(item.offer.heading())</strong>
                                                if item.offer.title.is_some() { <small>(&item.offer.name)</small> }
                                                if let Some(label) = &item.offer.variant_label { <small class="variant">(label)</small> }
                                                <span>
                                                    <s aria-label=(format!("au lieu de {}", item.price))>(&item.price)</s> " "
                                                    <strong class="price" data-offer-price="">(&item.offer_price)</strong> " "
                                                    <span class="off">"−" (item.percent_off) "%"</span>
                                                </span>
                                            </span>
                                            // The offer's own form sits after this one: forms do not nest.
                                            <button type="submit" form=(&item.form_id)
                                                aria-label=(format!("Ajouter {} à {}", item.offer.name, item.offer_price))>"Ajouter"</button>
                                        </li>
                                    }
                                </ul>
                            </section>
                        }
                        <section class="card">
                            <h2>"Paiement"</h2>
                            <p class="hint">"Toutes les transactions sont sécurisées et chiffrées. Les données de votre carte ne sont jamais stockées sur nos serveurs."</p>
                            if page.setup.is_some() {
                                payment_element()
                            } else {
                                <p class="hint" data-payments-off="">"Le paiement par carte n'est pas encore disponible dans cette boutique."</p>
                            }
                            payment_error(attrs: attributes! { class="notice" })
                            place_button(attrs: attributes! { class="primary place" },
                                "Payer ma commande · " <span data-place-total="">(&page.total)</span>
                            )
                        </section>
                    )
                    for item in &page.offers {
                        offer_form(offer: &item.offer)
                    }
                </div>
            </div>
            checkout_script(page: &page)
        )
    })
}
