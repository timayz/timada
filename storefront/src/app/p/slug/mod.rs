//! A product: its pictures, its price, the options to pick, the facts about
//! it, what shoppers said, and what goes with it.

use timada_storefront::{
    ProductPageLd, absolute_url, breadcrumb_ld, categories, category_path, menu_of, offer_terms,
    product_categories, product_image, product_ld, product_seo, related_products, require_product,
    reviews_of, stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, query_params},
    view::{View, attributes, view},
};

use crate::app::cart::add::{OPTION_FIELD_PREFIX, add};
use crate::app::{delivery_days, document, money, price_line, product_card, say};

module_param!(pub slug);

#[topcoat::router::query_params(error = bad_request)]
struct ProductPageQuery {
    /// A refusal from the cart, by code.
    error: Option<String>,
    /// The page of reviews, 1-based.
    reviews: Option<u32>,
    /// A variant's id: the page opens on it, with its price, its picture
    /// first and its values picked. The address a search engine lists that
    /// variant at.
    v: Option<String>,
}

#[derive(serde::Serialize)]
struct VariantLink<'a> {
    v: &'a str,
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
    let product = require_product(cx, slug(cx)).await?;
    let seo = product_seo(cx, &product).await?;
    let reviews = reviews_of(cx, &product, query.reviews.unwrap_or(1)).await?;
    let slug = product.slug.clone();
    let reviews_link = move |page: u32| {
        format!(
            "{}#reviews",
            href!(index, slug: slug.clone())
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
    let sold_out = !product.in_stock;
    // A product with nothing to pick says how many are left once few are:
    // of itself, or of the one variant on sale when every option is hidden.
    let few_left = product
        .left(product.variants.first().map(|variant| variant.id.as_str()))
        .filter(|left| product.options.is_empty() && (1..=5).contains(left));
    let currency = product.currency.clone().unwrap_or_else(|| "EUR".to_owned());
    // The first paragraph introduces the product beside its price; the
    // rest, when there is more, folds under "Description".
    let (intro, more): (String, String) = match product.description.split_once("\n\n") {
        Some((intro, more)) => (intro.trim().to_owned(), more.trim().to_owned()),
        None => (product.description.trim().to_owned(), String::new()),
    };
    // The variant the address names, when the product sells it. The view
    // keeps what it shows, so these are copies.
    let shown = query
        .v
        .as_deref()
        .and_then(|id| product.variants.iter().find(|variant| variant.id == id))
        .cloned();
    let shown_image = shown
        .as_ref()
        .and_then(|variant| variant.image_id.as_deref())
        .and_then(|id| product.images.iter().find(|image| image.id == id));
    // Its picture leads the gallery.
    let gallery: Vec<timada_storefront::Image> = shown_image
        .into_iter()
        .chain(
            product
                .images
                .iter()
                .filter(|image| shown_image.is_none_or(|shown| shown.id != image.id)),
        )
        .cloned()
        .collect();
    let shown_sold_out = shown
        .as_ref()
        .is_some_and(|variant| product.is_sold_out(Some(&variant.id)));
    // Each variant's own address, in the variants' order.
    let variant_paths: Vec<String> = product
        .variants
        .iter()
        .map(|variant| {
            href!(index, slug: product.slug.clone())
                .query(VariantLink { v: &variant.id })
                .resolve(cx)
        })
        .collect();

    // For search engines: one address for the product whatever variant or
    // page of reviews is open, and what this page shows as data.
    let path = href!(index, slug: product.slug.clone()).resolve(cx);
    let canonical = absolute_url(cx, &path);
    let home = absolute_url(cx, &href!(crate::app::index).resolve(cx));
    let trail: Vec<(String, String)> = crumbs
        .iter()
        .map(|crumb| {
            let path =
                href!(crate::app::c::slug::index, crate::app::c::slug::slug: crumb.slug.clone())
                    .resolve(cx);
            (crumb.name.clone(), absolute_url(cx, &path))
        })
        .collect();
    // What the shop says of delivery and returns, on every offer.
    let terms = offer_terms(cx).await?;
    let structured = [
        product_ld(
            &product,
            &ProductPageLd {
                url: &canonical,
                description: &seo.description,
                reviews: &reviews.items,
                terms: &terms,
            },
            |variant| {
                let position = product.variants.iter().position(|v| v.id == variant.id);
                absolute_url(cx, position.map_or(path.as_str(), |n| &variant_paths[n]))
            },
        ),
        breadcrumb_ld(
            [("Accueil", home.as_str())]
                .into_iter()
                .chain(
                    trail
                        .iter()
                        .map(|(name, url)| (name.as_str(), url.as_str())),
                )
                .chain([(product.name.as_str(), canonical.as_str())]),
        ),
    ];
    // A shared link shows the variant opened, else the product's first
    // picture and lowest price.
    let card_image = gallery.first().map(|image| image.url.clone());
    let card_price = match &shown {
        Some(variant) => Some((variant.amount_cents, variant.currency.clone())),
        None => product
            .min_amount_cents
            .map(|cents| (cents, currency.clone())),
    };

    let events = crate::app::tag::product_events(cx, &product, shown.as_ref()).await?;

    Ok(view! {
        document(
            title: &seo.title,
            events: Some(&events),
            description: Some(seo.description.as_str()).filter(|text| !text.is_empty()),
            canonical: Some(path.as_str()),
            image: card_image.as_deref(),
            price: card_price.as_ref().map(|(cents, currency)| (*cents, currency.as_str())),
            structured: &structured,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label="Fil d'Ariane" data-crumbs="">
                    <a href=(href!(crate::app::index))>"Accueil"</a>
                    for crumb in &crumbs {
                        " / "
                        <a href=(href!(crate::app::c::slug::index, crate::app::c::slug::slug: crumb.slug.clone()))>(&crumb.name)</a>
                    }
                    " / " (&product.name)
                </nav>
                <div class="product">
                    <div>
                        if !product.images.is_empty() {
                            <ul class="gallery" data-gallery="">
                                // The first spans the gallery: the page rendition, with
                                // the small one for phones; the others are tiles.
                                for (position, image) in gallery.iter().enumerate() {
                                    <li>product_image(image: image, large: position == 0, attrs: attributes! { class="photo" })</li>
                                }
                            </ul>
                        }
                    </div>
                    <div class="buy">
                        if let Some(brand) = &product.brand {
                            <span class="eyebrow" data-brand="">(brand)</span>
                        }
                        <h1 data-product=(&product.slug)>(&product.name)</h1>
                        <p class="price">
                            if let Some(variant) = &shown {
                                if let Some(was) = variant.compare_at_cents {
                                    <s class="was" data-compare-at=""><span class="unseen">"Ancien prix : "</span>(money(was, &variant.currency))</s>
                                }
                                <span data-price="" data-shown-variant=(&variant.id)>(money(variant.amount_cents, &variant.currency))</span>
                            } else {
                                if varies { <small>"Plage de prix"</small> }
                                if let (Some(was), Some(currency)) = (product.compare_at_cents, product.currency.as_deref()) {
                                    if product.variants.is_empty() {
                                        <s class="was" data-compare-at=""><span class="unseen">"Ancien prix : "</span>(money(was, currency))</s>
                                    }
                                }
                                <span data-price="">(price_line(&product))</span>
                            }
                        </p>
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
                        if sold_out || shown_sold_out {
                            <p class="stock" data-stock="sold-out">"Épuisé"</p>
                        } else if let Some(left) = few_left {
                            <p class="stock" data-stock=(left)>"Plus que " (left) " en stock"</p>
                        }
                        // Plain HTML: the menus pick the variant on the server, so a
                        // shopper with no JavaScript buys the same way; the table
                        // below prices every combination.
                        <form class="picker" method="post" action=(href!(add))>
                            <input type="hidden" name="slug" value=(&product.slug)>
                            for option in &product.options {
                                <p>
                                    <label for=(format!("option-{}", option.id))>(option.display_name())</label>
                                    <select id=(format!("option-{}", option.id)) name=(format!("{OPTION_FIELD_PREFIX}{}", option.id)) data-option=(&option.name)>
                                        for value in &option.values {
                                            <option value=(value) selected=(shown.as_ref().is_some_and(|variant| variant.value_of(&option.id) == Some(value.as_str())))>(option.display_value(value))</option>
                                        }
                                    </select>
                                </p>
                            }
                            <p>
                                <label for="quantity">"Quantité"</label>
                                <input id="quantity" name="quantity" type="number" min="1" max="99" value="1" aria-label=(format!("quantité de {}", product.name))>
                            </p>
                            <button class="btn" disabled=(sold_out)>"Ajouter au panier"</button>
                        </form>
                        <ul class="reassure-list" data-reassure="">
                            if let Some(days) = product.delivery_days {
                                <li data-delivery-days=(delivery_days(days))>"Livraison avec suivi en " (delivery_days(days))</li>
                            } else {
                                <li>"Livraison avec suivi"</li>
                            }
                            <li>"14 jours satisfait ou remboursé"</li>
                            <li>"Un service client à votre écoute"</li>
                        </ul>
                        if !product.options.is_empty() {
                            <details>
                                <summary>"Tarifs par option"</summary>
                                <table class="variants" data-variants="">
                                    <thead>
                                        <tr>
                                            for option in &product.options {
                                                <th scope="col">(option.display_name())</th>
                                            }
                                            <th scope="col">"Prix"</th>
                                            <th scope="col">"Stock"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        for (nth, variant) in product.variants.iter().enumerate() {
                                            <tr data-variant=(&variant.id) data-price=(variant.amount_cents) data-sold-out=(product.is_sold_out(Some(&variant.id)))>
                                                // The first cell links to the page opened on
                                                // this variant: its own address.
                                                for (position, option) in product.options.iter().enumerate() {
                                                    <td>
                                                        if position == 0 {
                                                            <a href=(&variant_paths[nth]) aria-current=((shown.as_ref().map(|shown| shown.id.as_str()) == Some(variant.id.as_str())).then_some("true"))>(variant.value_of(&option.id).map_or("—", |value| option.display_value(value)))</a>
                                                        } else {
                                                            (variant.value_of(&option.id).map_or("—", |value| option.display_value(value)))
                                                        }
                                                    </td>
                                                }
                                                <td class="price">(money(variant.amount_cents, &currency))</td>
                                                <td>
                                                    match product.left(Some(&variant.id)) {
                                                        Some(0) => "Épuisé",
                                                        Some(left) if left <= 5 => (format!("Plus que {left}")),
                                                        _ => "En stock",
                                                    }
                                                </td>
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
                                            <a href=(&photo.url)><img src=(&photo.thumb_url) width=(photo.thumb_width) height=(photo.thumb_height) loading="lazy" alt="Une photo du client" data-review-photo=(&photo.id)></a>
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
