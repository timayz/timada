//! `/t/{slug}/reviews/{review_id}`: one review, read from its own stream (so
//! a moderation shows at once), with its photos and the forms that publish,
//! reject or trim it.

pub mod publish;
pub mod reject;
pub mod remove_photo;

use std::sync::Arc;

use timada_reviews::{ReviewError, ReviewState, ReviewStatus, stars};
use timada_tenancy::{PERMISSION_REVIEWS_MANAGE, PERMISSION_REVIEWS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, view},
};

use super::{reviews, reviews_url, status_label};
use crate::app::auth::Flash;
use crate::app::t::slug::orders::order_id::{self, OrderId};
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::media::Media;
use crate::ui::{document, format_time, notice};

module_param!(pub review_id);

#[topcoat::router::query_params(error = bad_request)]
struct ReviewQuery {
    error: Option<String>,
    message: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
pub struct RejectForm {
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, serde::Deserialize)]
pub struct PhotoForm {
    pub image_id: String,
}

pub fn review_url(
    cx: &Cx,
    slug: &str,
    review_id: &str,
    error: Option<&str>,
    message: Option<&str>,
) -> String {
    href!(index, Slug(slug.to_owned()), ReviewId(review_id.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The review named in the URL, or 404.
async fn require_review(cx: &Cx, access: &TenantAccess) -> Result<ReviewState> {
    let id = path_param::<ReviewId>(cx);
    reviews(access)?
        .load_review(id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_else(|| not_found().into())
}

/// What every action on a review shares: the member may manage reviews, and
/// the outcome goes back to the review's page.
async fn act<F>(cx: &Cx, message: &'static str, run: F) -> Result<SeeOther>
where
    F: AsyncFnOnce(
        &timada_reviews::Command<crate::cluster::Executor>,
        &str,
        &str,
    ) -> std::result::Result<(), ReviewError>,
{
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_REVIEWS_MANAGE)
        .await?;
    let review_id = path_param::<ReviewId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    let gateway = reviews(&access)?;
    match run(&gateway, &access.user_id, &review_id).await {
        Ok(()) => Ok(see_other(review_url(
            cx,
            &slug,
            &review_id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(review_url(
            cx,
            &slug,
            &review_id,
            err.code(),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_REVIEWS_READ)
        .await?;
    let can_manage = access.can(cx, PERMISSION_REVIEWS_MANAGE).await?;
    let query = query_params::<ReviewQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let review = require_review(cx, &access).await?;
    let order = timada_orders::Command(access.executor()?.clone())
        .load_order(&review.order_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let (reference, product_name) = order
        .map(|order| {
            let name = order
                .lines
                .iter()
                .find(|line| line.product_id == review.product_id)
                .map(|line| line.name.clone())
                .unwrap_or_default();
            (order.reference, name)
        })
        .unwrap_or_default();
    let media = app_context::<Arc<Media>>(cx);
    let photos: Vec<(String, String)> = review
        .photos
        .iter()
        .map(|photo| {
            (
                photo.id.clone(),
                media.url(&access.tenant.id, &photo.file()),
            )
        })
        .collect();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let id = review.id.clone();
    let title = format!("Review of {product_name} · {}", tenant.name);
    let back = reviews_url(cx, &slug, None, None);
    let order_link = href!(
        order_id::index,
        Slug(slug.clone()),
        OrderId(review.order_id.clone())
    );
    let open = can_manage && !review.is_rejected();

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Review of " (&product_name)</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-4 text-sm"><a class="underline" href=(back)>"All reviews"</a></p>
                <dl class="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm">
                    <dt class="text-neutral-500">"Status"</dt>
                    <dd data-review-status=(review.status.as_str())>(status_label(review.status))</dd>
                    if let Some(reason) = review.rejection_reason.as_deref().filter(|reason| !reason.is_empty()) {
                        <dt class="text-neutral-500">"Why"</dt>
                        <dd data-review-reason="">(reason)</dd>
                    }
                    <dt class="text-neutral-500">"Stars"</dt>
                    <dd data-review-rating=(review.rating) aria-label=(format!("{} out of 5", review.rating))>(stars(review.rating))</dd>
                    <dt class="text-neutral-500">"Product"</dt>
                    <dd>
                        (&product_name)
                        if let Some(label) = &review.variant_label {
                            <span class="text-neutral-500">" · " (label)</span>
                        }
                    </dd>
                    <dt class="text-neutral-500">"Order"</dt>
                    <dd><a class="underline" href=(order_link)><code>(&reference)</code></a></dd>
                    <dt class="text-neutral-500">"Signed"</dt>
                    <dd>(&review.author_name)</dd>
                    <dt class="text-neutral-500">"Written"</dt>
                    <dd>(format_time(review.submitted_at))</dd>
                    if let Some(at) = review.published_at {
                        <dt class="text-neutral-500">"Published"</dt>
                        <dd>(format_time(at))</dd>
                    }
                </dl>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                if let Some(heading) = &review.title {
                    <h2 class="mb-2 text-lg font-medium" data-review-title="">(heading)</h2>
                }
                <p class="whitespace-pre-line text-sm" data-review-body="">(&review.body)</p>
                if !photos.is_empty() {
                    <ul class="mt-4 flex flex-wrap gap-4">
                        for (photo_id, url) in &photos {
                            <li class="space-y-2" data-review-photo=(photo_id)>
                                <a href=(url)><img class="h-40 w-40 rounded border border-neutral-200 object-cover" src=(url) alt="A photo the shopper attached"></a>
                                if open {
                                    <form method="post" action=(href!(remove_photo::remove_photo, Slug(slug.clone()), ReviewId(id.clone())))>
                                        <input type="hidden" name="image_id" value=(photo_id)>
                                        <button class="text-sm text-red-700 underline">"Remove this photo"</button>
                                    </form>
                                }
                            </li>
                        }
                    </ul>
                }
            </section>

            if open {
                <section class="space-y-4 rounded border border-neutral-200 bg-white p-4">
                    if review.status == ReviewStatus::Pending {
                        <form method="post" action=(href!(publish::publish, Slug(slug.clone()), ReviewId(id.clone())))>
                            <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Publish"</button>
                        </form>
                    }
                    <form method="post" action=(href!(reject::reject, Slug(slug.clone()), ReviewId(id.clone()))) class="space-y-2">
                        <label class="block text-sm" for="review-reason">"Why it is rejected (shown to the shopper, optional)"</label>
                        <input id="review-reason" name="reason" type="text" maxlength="300" class="w-full rounded border border-neutral-300 px-3 py-2">
                        <button class="rounded border border-red-300 px-4 py-2 font-medium text-red-700">
                            if review.is_published() { "Take down" } else { "Reject" }
                        </button>
                    </form>
                    <p class="text-sm text-neutral-500">"Rejecting is final: the review and its photos are gone from the shop for good."</p>
                </section>
            }
        )
    })
}
