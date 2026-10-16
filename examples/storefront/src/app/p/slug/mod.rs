use timada_storefront::{
    menu, price, product_categories, product_image, related_products, require_product, reviews_of,
    stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{href, module_param, page, path_param, query_params},
    view::{View, attributes, view},
};

use crate::app::cart::add::{OPTION_FIELD_PREFIX, add};
use crate::app::{catalog_grid, document, say};

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

/// How many products the "you may also like" row shows.
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
    let nav = menu(cx).await?;
    let related = related_products(cx, &product, RELATED).await?;
    // The first category it is filed under carries the breadcrumb.
    let crumb = product_categories(cx, &product).await?.into_iter().next();

    Ok(view! {
        document(
            title: &product.name,
            nav: &nav,
            <nav class="crumbs" aria-label="Breadcrumb" data-crumbs="">
                <a href=(href!(crate::app::index))>"Catalog"</a>
                if let Some(crumb) = &crumb {
                    " / "
                    <a href=(href!(crate::app::c::slug::index, crate::app::c::slug::Slug(crumb.slug.clone())))>(&crumb.name)</a>
                }
                " / " (&product.name)
            </nav>
            <h1 data-product=(&product.slug)>(&product.name)</h1>
            if !product.images.is_empty() {
                <ul class="gallery" data-gallery="">
                    for image in &product.images {
                        <li>product_image(image: image, attrs: attributes! { class="photo" })</li>
                    }
                </ul>
            }
            <p>price(product: &product, attrs: attributes! { class="price" })</p>
            if let (Some(rating), Some(whole)) = (product.rating(), product.rating_stars()) {
                <p class="rating" data-product-rating=(format!("{rating:.1}"))>
                    <a href="#reviews">
                        <span aria-hidden="true">(stars(whole))</span>
                        " " (format!("{rating:.1}")) " out of 5 · " (product.review_count) (if product.review_count == 1 { " review" } else { " reviews" })
                    </a>
                </p>
            }

            if let Some(code) = query.error.as_deref() {
                <p class="notice" role="alert">(say(code))</p>
            }
            // Plain HTML: the menus pick the variant on the server, so a
            // shopper with no JavaScript buys the same way; the table below
            // prices every combination.
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
                    <label for="quantity">"Quantity"</label>
                    <input id="quantity" name="quantity" type="number" min="1" max="99" value="1">
                </p>
                <button class="primary">"Add to cart"</button>
            </form>
            if !product.options.is_empty() {
                <table class="variants" data-variants="">
                    <thead>
                        <tr>
                            for option in &product.options {
                                <th scope="col">(&option.name)</th>
                            }
                            <th scope="col">"Price"</th>
                        </tr>
                    </thead>
                    <tbody>
                        for variant in &product.variants {
                            <tr data-variant=(&variant.id) data-price=(variant.amount_cents)>
                                for option in &product.options {
                                    <td>(variant.value_of(&option.id).unwrap_or("—"))</td>
                                }
                                <td class="price">(variant.price())</td>
                            </tr>
                        }
                    </tbody>
                </table>
            }

            <p class="description">(&product.description)</p>

            if !product.attributes.is_empty() {
                <h2>"Additional information"</h2>
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
            }
            <section class="reviews" id="reviews" data-reviews=(reviews.total)>
                <h2>"Reviews"</h2>
                if reviews.items.is_empty() {
                    <p>"No review yet. Bought it? Review it from your order's page."</p>
                }
                <ul>
                    for review in &reviews.items {
                        <li data-review=(&review.id)>
                            <p>
                                <span class="rating" aria-label=(format!("{} out of 5", review.rating))>(review.stars())</span>
                                if let Some(title) = &review.title {
                                    " " <strong>(title)</strong>
                                }
                            </p>
                            <p>(&review.body)</p>
                            if !review.photos.is_empty() {
                                <p>
                                    for photo in &review.photos {
                                        <a href=(&photo.url)><img src=(&photo.url) alt="A photo from the reviewer" data-review-photo=(&photo.id)></a>
                                    }
                                </p>
                            }
                            <p>
                                <small>
                                    (&review.author_name) " · Verified purchase"
                                    if let Some(label) = &review.variant_label { " · " (label) }
                                </small>
                            </p>
                            if let Some(reply) = &review.reply {
                                <blockquote class="review-reply" data-review-reply="">
                                    <p><strong>"Reply from the shop"</strong></p>
                                    <p>(reply)</p>
                                </blockquote>
                            }
                        </li>
                    }
                </ul>
                if reviews.pages() > 1 {
                    <nav class="pages" aria-label="Pages of reviews">
                        if reviews.page > 1 { <a href=(reviews_link(reviews.page - 1))>"Newer reviews"</a> }
                        <span>"Page " (reviews.page) " of " (reviews.pages())</span>
                        if reviews.page < reviews.pages() { <a href=(reviews_link(reviews.page + 1))>"Older reviews"</a> }
                    </nav>
                }
            </section>
            if !related.is_empty() {
                <section class="related">
                    <h2>"You may also like"</h2>
                    catalog_grid(products: &related)
                </section>
            }
        )
    })
}
