//! A product: its pictures, its price, the options to pick, the facts about
//! it, what shoppers said, and what goes with it.

use timada_storefront::{
    ProductPageLd, breadcrumb_ld, categories, menu_of, offer_terms, product_image, product_ld,
    product_seo, related_products, require_product, reviews_of, stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{error::redirect_permanent, href, module_param, page, query_params},
    view::{View, attributes, view},
};

use crate::app::cart::add::{OPTION_FIELD_PREFIX, add};
use crate::app::words::t;
use crate::app::{address, delivery_days, document, money, price_line, product_card, say};

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
    let paths = address::paths(cx).await?;
    // Its default address: under its main category, which is the one
    // search engines are told, or this route for a product filed nowhere.
    let path = address::product_href(cx, paths, &product);
    // The category of the path it was asked under, when it was.
    let under = address::via(cx).and_then(|via| via.category_id.clone());
    let here = match &under {
        Some(category_id) => address::product_href_under(cx, paths, category_id, &product),
        // One address a language, and one place for a product: asked at
        // this route though it is filed somewhere, or by another slug of
        // its own (the main one, where it is translated), it moves to its
        // default address.
        None if product.slug != slug(cx) || paths.product(&product).is_some() => {
            return Err(redirect_permanent(format!("{path}{}", address::query_of(cx))).into());
        }
        None => path.clone(),
    };
    let seo = product_seo(cx, &product).await?;
    let reviews = reviews_of(cx, &product, query.reviews.unwrap_or(1)).await?;
    let reviews_link = {
        let here = here.clone();
        move |page: u32| {
            format!(
                "{}#reviews",
                address::with_query(&here, &ReviewsLink { reviews: page })
            )
        }
    };
    let tree = categories(cx).await?;
    let nav = menu_of(&tree);
    let related = related_products(cx, &product, RELATED).await?;
    // The breadcrumb follows the address: the category it was asked
    // under, else its main one, with the categories above it.
    let crumbs = match under.as_deref().or_else(|| paths.main(&product.categories)) {
        Some(category_id) => address::trail(&tree, category_id),
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
    // The average as it is written, to one decimal.
    let average = product
        .rating()
        .and_then(|rating| format!("{rating:.1}").parse::<f64>().ok());
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
        .map(|variant| address::with_query(&here, &VariantLink { v: &variant.id }))
        .collect();
    // And the one search engines list it at, under the default address.
    let variant_listings: Vec<String> = product
        .variants
        .iter()
        .map(|variant| address::with_query(&path, &VariantLink { v: &variant.id }))
        .collect();

    // For search engines: one address for the product whatever category
    // it was reached from and whatever variant or page of reviews is open,
    // and what this page shows as data.
    let speaking = crate::app::lang::speaking(cx).await;
    let there = crate::app::lang::product_elsewhere(cx, &product).await?;
    let elsewhere = speaking.elsewhere(&path, |language| {
        crate::app::lang::there(&there, language, &path)
    });
    let canonical = speaking.absolute(cx, &path);
    let home = speaking.absolute(cx, &href!(crate::app::index).resolve(cx));
    let trail: Vec<(String, String)> = crumbs
        .iter()
        .map(|crumb| {
            let path = address::category_href(cx, paths, crumb);
            (crumb.name.clone(), speaking.absolute(cx, &path))
        })
        .collect();
    // What the shop says of delivery and returns, on every offer.
    let terms = offer_terms(cx).await?;
    let home_name = t!(shell_home).resolve(cx);
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
                speaking.absolute(cx, position.map_or(path.as_str(), |n| &variant_listings[n]))
            },
        ),
        breadcrumb_ld(
            [(home_name.as_ref(), home.as_str())]
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
            elsewhere: &elsewhere,
            image: card_image.as_deref(),
            price: card_price.as_ref().map(|(cents, currency)| (*cents, currency.as_str())),
            structured: &structured,
            nav: &nav,
            <div class="wrap">
                <nav class="crumbs" aria-label=(t!(shell_crumbs)) data-crumbs="">
                    <a href=(href!(crate::app::index))>(t!(shell_home))</a>
                    for crumb in &crumbs {
                        " / "
                        <a href=(address::category_href(cx, paths, crumb))>(&crumb.name)</a>
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
                                    <s class="was" data-compare-at=""><span class="unseen">(t!(price_former)) " "</span>(money(was, &variant.currency))</s>
                                }
                                <span data-price="" data-shown-variant=(&variant.id)>(money(variant.amount_cents, &variant.currency))</span>
                            } else {
                                if varies { <small>(t!(card_price_range))</small> }
                                if let (Some(was), Some(currency)) = (product.compare_at_cents, product.currency.as_deref()) {
                                    if product.variants.is_empty() {
                                        <s class="was" data-compare-at=""><span class="unseen">(t!(price_former)) " "</span>(money(was, currency))</s>
                                    }
                                }
                                <span data-price="">(price_line(cx, &product))</span>
                            }
                        </p>
                        if let (Some(rating), Some(average), Some(whole)) = (product.rating(), average, product.rating_stars()) {
                            <p class="rating" data-product-rating=(format!("{rating:.1}"))>
                                <a href="#reviews">
                                    <span class="stars" aria-hidden="true">(stars(whole))</span>
                                    " " (t!(product_rating, rating: average, count: product.review_count))
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
                            <p class="stock" data-stock="sold-out">(t!(product_sold_out))</p>
                        } else if let Some(left) = few_left {
                            <p class="stock" data-stock=(left)>(t!(product_few_left, left))</p>
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
                                <label for="quantity">(t!(product_quantity))</label>
                                <input id="quantity" name="quantity" type="number" min="1" max="99" value="1" aria-label=(t!(product_quantity.label, name: &product.name))>
                            </p>
                            <button class="btn" disabled=(sold_out)>(t!(card_add_to_cart))</button>
                        </form>
                        <ul class="reassure-list" data-reassure="">
                            if let Some(days) = product.delivery_days {
                                <li data-delivery-days=(delivery_days(days))>(t!(product_delivery_within, days: delivery_days(days).resolve(cx)))</li>
                            } else {
                                <li>(t!(product_delivery))</li>
                            }
                            <li>(t!(product_money_back))</li>
                            <li>(t!(product_service))</li>
                        </ul>
                        if !product.options.is_empty() {
                            <details>
                                <summary>(t!(product_variants))</summary>
                                <table class="variants" data-variants="">
                                    <thead>
                                        <tr>
                                            for option in &product.options {
                                                <th scope="col">(option.display_name())</th>
                                            }
                                            <th scope="col">(t!(product_variants_price))</th>
                                            <th scope="col">(t!(product_variants_stock))</th>
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
                                                        Some(0) => (t!(product_sold_out)),
                                                        Some(left) if left <= 5 => (t!(product_variant_few_left, left)),
                                                        _ => (t!(product_variant_in_stock)),
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
                                <summary>(t!(product_description))</summary>
                                <p class="description">(&more)</p>
                            </details>
                        }
                        if !product.attributes.is_empty() {
                            <details>
                                <summary>(t!(product_facts))</summary>
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
                    <h2>(t!(product_reviews))</h2>
                    if reviews.items.is_empty() {
                        <p>(t!(product_reviews_none))</p>
                    }
                    <ul>
                        for review in &reviews.items {
                            <li data-review=(&review.id)>
                                <p>
                                    <span class="stars" aria-label=(t!(rating_of_five, rating: review.rating))>(review.stars())</span>
                                    if let Some(title) = &review.title {
                                        " " <strong>(title)</strong>
                                    }
                                </p>
                                <p>(&review.body)</p>
                                if !review.photos.is_empty() {
                                    <p>
                                        for photo in &review.photos {
                                            <a href=(&photo.url)><img src=(&photo.thumb_url) width=(photo.thumb_width) height=(photo.thumb_height) loading="lazy" alt=(t!(product_review_photo)) data-review-photo=(&photo.id)></a>
                                        }
                                    </p>
                                }
                                <p>
                                    <small>
                                        (&review.author_name) " · " (t!(review_verified))
                                        if let Some(label) = &review.variant_label { " · " (label) }
                                    </small>
                                </p>
                                if let Some(reply) = &review.reply {
                                    <blockquote class="review-reply" data-review-reply="">
                                        <p><strong>(t!(product_review_reply))</strong></p>
                                        <p>(reply)</p>
                                    </blockquote>
                                }
                            </li>
                        }
                    </ul>
                    if reviews.pages() > 1 {
                        <nav class="pages" aria-label=(t!(product_reviews.pages))>
                            if reviews.page > 1 { <a href=(reviews_link(reviews.page - 1))>(t!(product_reviews_newer))</a> }
                            <span>(t!(product_reviews_page, page: reviews.page, pages: reviews.pages()))</span>
                            if reviews.page < reviews.pages() { <a href=(reviews_link(reviews.page + 1))>(t!(product_reviews_older))</a> }
                        </nav>
                    }
                </section>
                if !related.is_empty() {
                    <section class="related" data-section="similaires">
                        <h2>(t!(product_related))</h2>
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
