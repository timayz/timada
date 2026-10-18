//! `/t/{slug}/reviews`: what shoppers wrote about the products they bought,
//! for members with `reviews:read`; those with `reviews:manage` publish and
//! reject them. The list comes from the shared read model (a moment behind)
//! and opens on the reviews waiting; a review's own page reads its stream.
//! Above it, whether and when the buyers of fulfilled orders are mailed a
//! link to review what they bought.

pub mod requests;
pub mod review_id;

use timada_reviews::{
    Command, DEFAULT_REQUEST_DELAY_DAYS, MAX_REQUEST_DELAY_DAYS, ReviewFilter, ReviewStatus,
    count_reviews, reviews_of_tenant, stars,
};
use timada_tenancy::{PERMISSION_REVIEWS_MANAGE, PERMISSION_REVIEWS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::db::ReadPool;
use crate::ui::{Shell, document, format_time, notice};

const PAGE_SIZE: u32 = 50;
/// The `status` of the list showing every review.
const ALL: &str = "all";

#[topcoat::router::query_params(error = bad_request)]
struct ReviewsQuery {
    /// `pending` (the default), `published`, `rejected` or `all`.
    status: Option<String>,
    /// 1 to 5; anything else is any.
    rating: Option<String>,
    /// A product id.
    product: Option<String>,
    /// 1-based.
    page: Option<u32>,
    error: Option<String>,
    message: Option<String>,
}

#[derive(serde::Serialize)]
struct ReviewsLink<'a> {
    status: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    rating: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    product: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    page: Option<u32>,
}

pub fn reviews_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The list of a product's reviews, every status.
pub fn product_reviews_url(cx: &Cx, slug: &str, product_id: &str) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(ReviewsLink {
            status: ALL,
            rating: None,
            product: Some(product_id),
            page: None,
        })
        .resolve(cx)
}

/// The reviews gateway over the tenant's own store (403 while suspended).
pub fn reviews(access: &TenantAccess) -> Result<Command<Executor>> {
    Ok(Command(access.executor()?.clone()))
}

/// The word for where a review stands.
pub fn status_label(status: ReviewStatus) -> &'static str {
    match status {
        ReviewStatus::Pending => "Waiting",
        ReviewStatus::Published => "Published",
        ReviewStatus::Rejected => "Rejected",
    }
}

fn status_of(row_status: &str) -> &'static str {
    ReviewStatus::parse(row_status).map_or("Unknown", status_label)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_REVIEWS_READ)
        .await?;
    let query = query_params::<ReviewsQuery>(cx)?;
    let status = match query.status.as_deref() {
        Some(ALL) => None,
        Some(value) => Some(ReviewStatus::parse(value).unwrap_or_default()),
        None => Some(ReviewStatus::Pending),
    };
    let rating = query
        .rating
        .as_deref()
        .and_then(|value| value.parse::<u8>().ok())
        .filter(|value| (1..=5).contains(value));
    let product = query
        .product
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let page_number = query.page.unwrap_or(1).max(1);
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_REVIEWS_MANAGE).await?;
    let ask_after = reviews(&access)?
        .review_requests()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .delay_days();
    let has_storefront = crate::app::t::slug::orders::orders(&access)?
        .storefront_address()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .is_some();
    let TenantAccess { tenant, .. } = access;
    let pool = app_context::<ReadPool>(cx);
    let filter = ReviewFilter {
        status,
        rating,
        product_id: product.map(str::to_owned),
    };
    let total = count_reviews(pool, &tenant.id, &filter)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let rows = reviews_of_tenant(
        pool,
        &tenant.id,
        &filter,
        PAGE_SIZE,
        (page_number - 1) * PAGE_SIZE,
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let pages = total.div_ceil(u64::from(PAGE_SIZE)) as u32;
    let status_value = status.map_or(ALL, ReviewStatus::as_str);
    // Filtering by product keeps its name, from any of its rows.
    let product_name = product.and_then(|id| {
        rows.iter()
            .find(|row| row.product_id == id)
            .map(|row| row.product_name.clone())
    });
    let slug = tenant.slug.clone();
    let page_link = {
        let slug = slug.clone();
        move |page: u32| {
            href!(index, Slug(slug.clone()))
                .query(ReviewsLink {
                    status: status_value,
                    rating,
                    product,
                    page: Some(page),
                })
                .resolve(cx)
        }
    };
    let title = format!("Reviews · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "reviews"),
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Reviews"</h1>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4" data-review-requests=(ask_after.map_or("off".to_owned(), |days| days.to_string()))>
                <h2 class="mb-2 text-lg font-medium">"Review requests"</h2>
                <p class="mb-4 text-sm text-neutral-500">
                    "Some days after an order is marked fulfilled, its buyer gets a mail from the shop with a link to review what they bought. The link opens their order on the storefront for 60 days."
                </p>
                if !has_storefront {
                    <p class="mb-4 text-sm" data-review-requests-no-storefront="">
                        "No mail goes out until the storefront address is set on the "
                        <a class="underline" href=(href!(crate::app::t::slug::api_keys::index, Slug(slug.clone())))>"API keys page"</a> "."
                    </p>
                }
                if can_manage {
                    <form method="post" action=(href!(requests::requests, Slug(slug.clone()))) class="flex flex-wrap items-end gap-2">
                        <label class="text-sm">
                            <span class="mb-1 block text-neutral-500">"Ask buyers"</span>
                            <select name="ask" class="rounded border border-neutral-300 px-3 py-2">
                                <option value="on" selected=(ask_after.is_some())>"Yes"</option>
                                <option value="off" selected=(ask_after.is_none())>"Never"</option>
                            </select>
                        </label>
                        <label class="text-sm">
                            <span class="mb-1 block text-neutral-500">"Days after fulfilment"</span>
                            <input type="number" name="days" min="0" max=(MAX_REQUEST_DELAY_DAYS)
                                value=(ask_after.unwrap_or(DEFAULT_REQUEST_DELAY_DAYS))
                                class="w-24 rounded border border-neutral-300 px-3 py-2">
                        </label>
                        <button class="rounded bg-neutral-900 px-3 py-2 text-white">"Save"</button>
                    </form>
                } else {
                    <p class="text-sm">
                        match ask_after {
                            Some(days) => { "Buyers are asked " (days) " day(s) after fulfilment." }
                            None => { "Buyers are not asked." }
                        }
                    </p>
                }
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm text-neutral-500">
                    "A shopper reviews a product from the page of the order they bought it with, or from the link the review request mailed them. Nothing shows on the shop until it is published here; a rejected review never comes back."
                </p>
                <form method="get" class="mb-4 flex flex-wrap gap-2">
                    if let Some(product) = product {
                        <input type="hidden" name="product" value=(product)>
                    }
                    <label class="sr-only" for="reviews-status">"Status"</label>
                    <select id="reviews-status" name="status" class="rounded border border-neutral-300 px-3 py-2">
                        for option in ReviewStatus::ALL {
                            <option value=(option.as_str()) selected=(status == Some(option))>(status_label(option))</option>
                        }
                        <option value=(ALL) selected=(status.is_none())>"Every status"</option>
                    </select>
                    <label class="sr-only" for="reviews-rating">"Stars"</label>
                    <select id="reviews-rating" name="rating" class="rounded border border-neutral-300 px-3 py-2">
                        <option value="">"Any stars"</option>
                        for stars_count in (1u8..=5).rev() {
                            <option value=(stars_count) selected=(rating == Some(stars_count))>(stars(stars_count))</option>
                        }
                    </select>
                    <button class="rounded border border-neutral-300 px-3 py-2">"Filter"</button>
                </form>
                if let Some(product) = product {
                    <p class="mb-4 text-sm" data-reviews-product=(product)>
                        "Reviews of " (product_name.unwrap_or_else(|| "one product".to_owned())) ". "
                        <a class="underline" href=(reviews_url(cx, &slug, None, None))>"Every product"</a>
                    </p>
                }
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Product"</th>
                            <th class="py-2 font-medium">"Stars"</th>
                            <th class="py-2 font-medium">"Review"</th>
                            <th class="py-2 font-medium">"By"</th>
                            <th class="py-2 font-medium">"Status"</th>
                            <th class="py-2 font-medium">"Written"</th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if rows.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="6">"No review here."</td></tr>
                        }
                        for row in &rows {
                            <tr data-review=(&row.id) data-review-status=(&row.status)>
                                <td class="py-2">
                                    (&row.product_name)
                                    if let Some(label) = &row.variant_label {
                                        <span class="text-neutral-500">" · " (label)</span>
                                    }
                                </td>
                                <td class="py-2" aria-label=(format!("{} out of 5", row.rating))>(stars(row.rating as u8))</td>
                                <td class="py-2">
                                    <a class="underline" href=(href!(review_id::index, Slug(slug.clone()), review_id::ReviewId(row.id.clone())))>
                                        (row.title.clone().unwrap_or_else(|| excerpt(&row.body)))
                                    </a>
                                    if row.photo_count > 0 {
                                        <span class="text-neutral-500">" · " (row.photo_count) " photo(s)"</span>
                                    }
                                </td>
                                <td class="py-2">(&row.author_name)</td>
                                <td class="py-2">(status_of(&row.status))</td>
                                <td class="py-2"><time datetime=(row.submitted_at)>(format_time(row.submitted_at as u64))</time></td>
                            </tr>
                        }
                    </tbody>
                </table>
                if pages > 1 {
                    <p class="mt-4 flex items-center gap-4 text-sm">
                        if page_number > 1 { <a class="underline" href=(page_link(page_number - 1))>"Previous"</a> }
                        <span>"Page " (page_number) " of " (pages)</span>
                        if page_number < pages { <a class="underline" href=(page_link(page_number + 1))>"Next"</a> }
                    </p>
                }
            </section>
        )
    })
}

/// The start of a text, for a row that has no title.
fn excerpt(body: &str) -> String {
    const LEN: usize = 60;
    if body.chars().count() <= LEN {
        return body.to_owned();
    }
    let cut: String = body.chars().take(LEN).collect();
    format!("{}…", cut.trim_end())
}
