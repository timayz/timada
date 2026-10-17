//! A product: its pictures, its price, the options to pick, the facts about
//! it, what shoppers said, and what goes with it.

use timada_storefront::{
    categories, category_path, menu_of, product_categories, product_image, related_products,
    require_product, reviews_of, stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use crate::app::cart::add::{OPTION_FIELD_PREFIX, add};
use crate::app::{PROMO_CODE, document, money, price_line, product_card, say};

module_param!(pub slug);

#[topcoat::router::query_params(error = bad_request)]
struct ProductPageQuery {
    /// A refusal from the cart, by code.
    error: Option<String>,
    /// The page of reviews, 1-based.
    reviews: Option<u32>,
}

#[derive(serde::Serialize)]
struct ReviewsLink {
    reviews: u32,
}

/// How many products the "Produits similaires" row shows.
const RELATED: u32 = 4;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query_params::<ProductPageQuery>(cx)?;
    let product = require_product(cx, path_param::<Slug>(cx)).await?;
    let reviews = reviews_of(cx, &product, query.reviews.unwrap_or(1)).await?;
    let slug = product.slug.clone();
    let reviews_link = move |page: u32| {
        format!(
            "{}#reviews",
            href!(index, Slug(slug.clone()))
                .query(ReviewsLink { reviews: page })
                .resolve(cx)
        )
    };
    let tree = categories(cx).await?;
    let nav = menu_of(&tree);
    let related = related_products(cx, &product, RELATED).await?;
    // The first category it is filed under carries the breadcrumb, with
    // the categories above it.
    let crumbs = match product_categories(cx, &product).await?.into_iter().next() {
        Some(category) => {
            let mut path = category_path(&tree, &category);
            path.push(category);
            path
        }
        None => Vec::new(),
    };
    let varies = product.price_range().is_some_and(|range| range.varies());
    let currency = product.currency.clone().unwrap_or_else(|| "EUR".to_owned());
    // The first paragraph introduces the product beside its price; the
    // rest, when there is more, folds under "Description".
    let (intro, more): (String, String) = match product.description.split_once("\n\n") {
        Some((intro, more)) => (intro.trim().to_owned(), more.trim().to_owned()),
        None => (product.description.trim().to_owned(), String::new()),
    };

    Ok(view! {
        document(
            title: &product.name,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a>
                    for crumb in &crumbs {
                        " / "
                        <a href=(href!(crate::app::c::slug::index, crate::app::c::slug::Slug(crumb.slug.clone())))>(&crumb.name)</a>
                    }
                    " / " (&product.name)
                </nav>
                <div class="product">
                    <div>
                        if !product.images.is_empty() {
                            <ul class="gallery" data-gallery="">
                                for image in &product.images {
                                    <li>product_image(image: image, attrs: attributes! { class="photo" })</li>
                                }
                            </ul>
                        }
                    </div>
                    <div class="buy">
                        <h1 data-product=(&product.slug)>(&product.name)</h1>
                        <p class="price">
                            if varies { <small>"Plage de prix"</small> }
                            <span data-price="">(price_line(&product))</span>
                        </p>
                        <p class="promo-line">"-10% avec le code " <strong>(PROMO_CODE)</strong></p>
                        if let (Some(rating), Some(whole)) = (product.rating(), product.rating_stars()) {
                            <p class="rating" data-product-rating=(format!("{rating:.1}"))>
                                <a href="#reviews">
                                    <span class="stars" aria-hidden="true">(stars(whole))</span>
                                    " " (format!("{rating:.1}").replace('.', ",")) "/5 · " (product.review_count) " avis"
                                </a>
                            </p>
                        }
                        if !intro.is_empty() {
                            <p class="description" data-description="">(&intro)</p>
                        }

                        if let Some(code) = query.error.as_deref() {
                            <p class="notice" role="alert">(say(code))</p>
                        }
                        // Plain HTML: the menus pick the variant on the server, so a
                        // shopper with no JavaScript buys the same way; the table
                        // below prices every combination.
                        <form class="picker" method="post" action=(href!(add))>
                            <input type="hidden" name="slug" value=(&product.slug)>
                            for option in &product.options {
                                <p>
                                    <label for=(format!("option-{}", option.id))>(&option.name)</label>
                                    <select id=(format!("option-{}", option.id)) name=(format!("{OPTION_FIELD_PREFIX}{}", option.id)) data-option=(&option.name)>
                                        for value in &option.values {
                                            <option value=(value)>(value)</option>
                                        }
                                    </select>
                                </p>
                            }
                            <p>
                                <label for="quantity">"Quantité"</label>
                                <input id="quantity" name="quantity" type="number" min="1" max="99" value="1" aria-label=(format!("quantité de {}", product.name))>
                            </p>
                            <button class="btn">"Ajouter au panier"</button>
                        </form>
                        <ul class="reassure-list" data-reassure="">
                            <strong>"Livraison gratuite dès 65€ d'achats"</strong>
                            <li>"Livraison avec suivi en France, Belgique, Suisse"</li>
                            <li>"14 jours satisfait ou remboursé"</li>
                            <li>"Service client disponible 6/7J"</li>
                        </ul>
                        if !product.options.is_empty() {
                            <details>
                                <summary>"Tarifs par option"</summary>
                                <table class="variants" data-variants="">
                                    <thead>
                                        <tr>
                                            for option in &product.options {
                                                <th scope="col">(&option.name)</th>
                                            }
                                            <th scope="col">"Prix"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        for variant in &product.variants {
                                            <tr data-variant=(&variant.id) data-price=(variant.amount_cents)>
                                                for option in &product.options {
                                                    <td>(variant.value_of(&option.id).unwrap_or("—"))</td>
                                                }
                                                <td class="price">(money(variant.amount_cents, &currency))</td>
                                            </tr>
                                        }
                                    </tbody>
                                </table>
                            </details>
                        }
                        if !more.is_empty() {
                            <details open="">
                                <summary>"Description"</summary>
                                <p class="description">(&more)</p>
                            </details>
                        }
                        if !product.attributes.is_empty() {
                            <details>
                                <summary>"Informations complémentaires"</summary>
                                <table class="attributes" data-attributes="">
                                    <tbody>
                                        for attribute in &product.attributes {
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
                <section class="reviews" id="reviews" data-reviews=(reviews.total)>
                    <h2>"Avis de nos clients"</h2>
                    if reviews.items.is_empty() {
                        <p>"Aucun avis pour le moment. Vous l'avez acheté ? Donnez votre avis depuis la page de votre commande."</p>
                    }
                    <ul>
                        for review in &reviews.items {
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
                    if reviews.pages() > 1 {
                        <nav class="pages" aria-label="Pages d'avis">
                            if reviews.page > 1 { <a href=(reviews_link(reviews.page - 1))>"Avis plus récents"</a> }
                            <span>"Page " (reviews.page) " sur " (reviews.pages())</span>
                            if reviews.page < reviews.pages() { <a href=(reviews_link(reviews.page + 1))>"Avis plus anciens"</a> }
                        </nav>
                    }
                </section>
                if !related.is_empty() {
                    <section class="related" data-section="similaires">
                        <h2>"Produits similaires"</h2>
                        <ul class="rail four">
                            for product in &related {
                                product_card(product: product)
                            }
                        </ul>
                    </section>
                }
            </div>
        )
    })
}
