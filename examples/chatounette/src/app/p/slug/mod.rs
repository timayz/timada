//! A product: its pictures, its price, the options to pick, the facts about
//! it, what shoppers said, and what goes with it.

use timada_storefront::{
    add_to_cart_form, option_field,
    pages::{ProductOptions, product_page},
    price, product_image, stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{module_param, page, path_param},
    view::{View, attributes, view},
};

use crate::app::{PROMO_CODE, document, product_card};

module_param!(pub slug);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let page = product_page(cx, path_param::<Slug>(cx), &ProductOptions::default()).await?;

    Ok(view! {
        document(
            title: &page.product.name,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(&page.home_url)>"Accueil"</a>
                    for crumb in &page.crumbs {
                        " / "
                        <a href=(&crumb.url)>(&crumb.name)</a>
                    }
                    " / " (&page.product.name)
                </nav>
                <div class="product">
                    <div>
                        if !page.product.images.is_empty() {
                            <ul class="gallery" data-gallery="">
                                for image in &page.product.images {
                                    <li>product_image(image: image, attrs: attributes! { class="photo" })</li>
                                }
                            </ul>
                        }
                    </div>
                    <div class="buy">
                        <h1 data-product=(&page.product.slug)>(&page.product.name)</h1>
                        <p class="price">
                            if page.varies { <small>"Plage de prix"</small> }
                            price(product: &page.product)
                        </p>
                        <p class="promo-line">"-10% avec le code " <strong>(PROMO_CODE)</strong></p>
                        if let (Some(rating), Some(whole), Some(text)) = (page.product.rating(), page.product.rating_stars(), &page.rating) {
                            <p class="rating" data-product-rating=(format!("{rating:.1}"))>
                                <a href="#reviews">
                                    <span class="stars" aria-hidden="true">(stars(whole))</span>
                                    " " (text) "/5 · " (page.product.review_count) " avis"
                                </a>
                            </p>
                        }
                        if !page.intro.is_empty() {
                            <p class="description" data-description="">(&page.intro)</p>
                        }

                        if let Some(error) = &page.error {
                            <p class="notice" role="alert">(&error.text)</p>
                        }
                        // Plain HTML: the menus pick the variant on the server, so a
                        // shopper with no JavaScript buys the same way; the table
                        // below prices every combination.
                        add_to_cart_form(product: &page.product, attrs: attributes! { class="picker" },
                            for option in &page.product.options {
                                <p>
                                    <label for=(format!("option-{}", option.id))>(&option.name)</label>
                                    <select id=(format!("option-{}", option.id)) name=(option_field(option)) data-option=(&option.name)>
                                        for value in &option.values {
                                            <option value=(value)>(value)</option>
                                        }
                                    </select>
                                </p>
                            }
                            <p>
                                <label for="quantity">"Quantité"</label>
                                <input id="quantity" name="quantity" type="number" min="1" max="99" value="1" aria-label=(format!("quantité de {}", page.product.name))>
                            </p>
                            <button class="btn">"Ajouter au panier"</button>
                        )
                        <ul class="reassure-list" data-reassure="">
                            <strong>"Livraison gratuite dès 65€ d'achats"</strong>
                            <li>"Livraison avec suivi en France, Belgique, Suisse"</li>
                            <li>"14 jours satisfait ou remboursé"</li>
                            <li>"Service client disponible 6/7J"</li>
                        </ul>
                        if !page.product.options.is_empty() {
                            <details>
                                <summary>"Tarifs par option"</summary>
                                <table class="variants" data-variants="">
                                    <thead>
                                        <tr>
                                            for option in &page.product.options {
                                                <th scope="col">(&option.name)</th>
                                            }
                                            <th scope="col">"Prix"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        for priced in &page.variants {
                                            <tr data-variant=(&priced.variant.id) data-price=(priced.variant.amount_cents)>
                                                for option in &page.product.options {
                                                    <td>(priced.variant.value_of(&option.id).unwrap_or("—"))</td>
                                                }
                                                <td class="price">(&priced.price)</td>
                                            </tr>
                                        }
                                    </tbody>
                                </table>
                            </details>
                        }
                        if !page.more.is_empty() {
                            <details open="">
                                <summary>"Description"</summary>
                                <p class="description">(&page.more)</p>
                            </details>
                        }
                        if !page.product.attributes.is_empty() {
                            <details>
                                <summary>"Informations complémentaires"</summary>
                                <table class="attributes" data-attributes="">
                                    <tbody>
                                        for attribute in &page.product.attributes {
                                            <tr data-attribute=(&attribute.name)>
                                                <th scope="row">(&attribute.name)</th>
                                                <td>(&attribute.value)</td>
                                            </tr>
                                        }
                                    </tbody>
                                </table>
                            </details>
                        }
                    </div>
                </div>
                <section class="reviews" id="reviews" data-reviews=(page.reviews.total)>
                    <h2>"Avis de nos clients"</h2>
                    if page.reviews.items.is_empty() {
                        <p>"Aucun avis pour le moment. Vous l'avez acheté ? Donnez votre avis depuis la page de votre commande."</p>
                    }
                    <ul>
                        for review in &page.reviews.items {
                            <li data-review=(&review.id)>
                                <p>
                                    <span class="stars" aria-label=(format!("{} sur 5", review.rating))>(review.stars())</span>
                                    if let Some(title) = &review.title {
                                        " " <strong>(title)</strong>
                                    }
                                </p>
                                <p>(&review.body)</p>
                                if !review.photos.is_empty() {
                                    <p>
                                        for photo in &review.photos {
                                            <a href=(&photo.url)><img src=(&photo.url) alt="Une photo du client" data-review-photo=(&photo.id)></a>
                                        }
                                    </p>
                                }
                                <p>
                                    <small>
                                        (&review.author_name) " · Achat vérifié"
                                        if let Some(label) = &review.variant_label { " · " (label) }
                                    </small>
                                </p>
                                if let Some(reply) = &review.reply {
                                    <blockquote class="review-reply" data-review-reply="">
                                        <p><strong>"Réponse de la boutique"</strong></p>
                                        <p>(reply)</p>
                                    </blockquote>
                                }
                            </li>
                        }
                    </ul>
                    if page.reviews.pages() > 1 {
                        <nav class="pages" aria-label="Pages d'avis">
                            if let Some(prev) = &page.reviews_prev_url { <a href=(prev)>"Avis plus récents"</a> }
                            <span>"Page " (page.reviews.page) " sur " (page.reviews.pages())</span>
                            if let Some(next) = &page.reviews_next_url { <a href=(next)>"Avis plus anciens"</a> }
                        </nav>
                    }
                </section>
                if !page.related.is_empty() {
                    <section class="related" data-section="similaires">
                        <h2>"Produits similaires"</h2>
                        <ul class="rail four">
                            for product in &page.related {
                                product_card(product: product)
                            }
                        </ul>
                    </section>
                }
            </div>
        )
    })
}
