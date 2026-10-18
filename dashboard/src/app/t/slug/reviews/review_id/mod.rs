//! `/t/{slug}/reviews/{review_id}`: one review, read from its own stream (so
//! a moderation shows at once), with its photos and the forms that publish,
//! reject or trim it, and answer it in public.

pub mod publish;
pub mod reject;
pub mod remove_photo;
pub mod remove_reply;
pub mod reply;

use std::sync::Arc;

use timada_reviews::{MAX_REPLY_LEN, ReviewError, ReviewState, ReviewStatus, stars};
use timada_tenancy::{PERMISSION_REVIEWS_MANAGE, PERMISSION_REVIEWS_READ};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, not_found, see_other},
        href, module_param, page, path_param, query_params,
    },
    view::{View, attributes, class, view},
};

use super::{reviews, reviews_url, status_label};
use crate::app::auth::Flash;
use crate::app::t::slug::orders::order_id::{self, OrderId};
use crate::app::t::slug::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::textarea::textarea;
use crate::media::Media;
use crate::ui::{
    LINK, Shell, Tone, document, fact, facts, format_time, notice, page_header, section_card,
    status_badge,
};

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
pub struct ReplyForm {
    #[serde(default)]
    pub body: String,
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
    let heading = format!("Review of {product_name}");

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "reviews"),
            page_header(
                title: &heading,
                subtitle: Some("One review, read from its own record."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(back)>"All reviews"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                facts(
                    fact(
                        term: "Status",
                        attrs: attributes! { data-review-status=(review.status.as_str()) },
                        status_badge(tone: review_tone(review.status), (status_label(review.status)))
                    )
                    if let Some(reason) = review.rejection_reason.as_deref().filter(|reason| !reason.is_empty()) {
                        fact(term: "Why", attrs: attributes! { data-review-reason="" }, (reason))
                    }
                    fact(
                        term: "Stars",
                        attrs: attributes! { data-review-rating=(review.rating) aria-label=(format!("{} out of 5", review.rating)) },
                        (stars(review.rating))
                    )
                    fact(
                        term: "Product",
                        (&product_name)
                        if let Some(label) = &review.variant_label {
                            <span class="text-muted-foreground">" · " (label)</span>
                        }
                    )
                    fact(term: "Order", <a class=(class!(LINK, "font-mono")) href=(order_link)><code>(&reference)</code></a>)
                    fact(term: "Signed", (&review.author_name))
                    fact(term: "Written", (format_time(review.submitted_at)))
                    if let Some(at) = review.published_at {
                        fact(term: "Published", (format_time(at)))
                    }
                )
            )

            section_card(
                if let Some(heading) = &review.title {
                    <h2 class="mb-3 text-base font-semibold" data-review-title="">(heading)</h2>
                }
                <p class="whitespace-pre-line text-sm" data-review-body="">(&review.body)</p>
                if !photos.is_empty() {
                    <ul class="mt-4 flex flex-wrap gap-4">
                        for (photo_id, url) in &photos {
                            <li class="flex flex-col gap-2" data-review-photo=(photo_id)>
                                <a href=(url)><img class="h-40 w-40 rounded-lg border border-border object-cover" src=(url) alt="A photo the shopper attached"></a>
                                if open {
                                    <form method="post" action=(href!(remove_photo::remove_photo, Slug(slug.clone()), ReviewId(id.clone())))>
                                        <input type="hidden" name="image_id" value=(photo_id)>
                                        button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Remove this photo")
                                    </form>
                                }
                            </li>
                        }
                    </ul>
                }
            )

            if review.reply.is_some() || open {
                section_card(
                    title: Some("Your public reply"),
                    description: Some(if review.reply.is_some() { "Shown under the review on the shop while it is published." } else { "Shown under the review on the shop once it is published." }),
                    attrs: attributes! { data-review-reply-section="" },
                    <div class="flex flex-col gap-4">
                        if let Some(reply) = &review.reply {
                            <p class="whitespace-pre-line text-sm" data-review-reply="">(reply)</p>
                            if let Some(at) = review.replied_at {
                                <p class="text-sm text-muted-foreground">"Written " (format_time(at))</p>
                            }
                        }
                        if open {
                            <form method="post" action=(href!(reply::reply, Slug(slug.clone()), ReviewId(id.clone()))) class="flex max-w-xl flex-col gap-4">
                                field(
                                    field_label(attrs: attributes! { for="review-reply" }, "Reply")
                                    textarea(attrs: attributes! { id="review-reply" name="body" rows="4" maxlength=(MAX_REPLY_LEN) required="" }, (review.reply.as_deref().unwrap_or_default()))
                                )
                                <div>
                                    button(
                                        attrs: attributes! { type="submit" },
                                        if review.reply.is_some() { "Update the reply" } else { "Post the reply" }
                                    )
                                </div>
                            </form>
                        }
                        if can_manage && review.reply.is_some() {
                            <form method="post" action=(href!(remove_reply::remove_reply, Slug(slug.clone()), ReviewId(id.clone())))>
                                button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Delete the reply")
                            </form>
                        }
                    </div>
                )
            }

            if open {
                section_card(
                    title: Some("Moderation"),
                    description: Some("Rejecting is final: the review and its photos are gone from the shop for good."),
                    <div class="flex flex-col gap-4">
                        if review.status == ReviewStatus::Pending {
                            <form method="post" action=(href!(publish::publish, Slug(slug.clone()), ReviewId(id.clone())))>
                                button(attrs: attributes! { type="submit" }, "Publish")
                            </form>
                        }
                        <form method="post" action=(href!(reject::reject, Slug(slug.clone()), ReviewId(id.clone()))) class="flex max-w-xl flex-col gap-4">
                            field(
                                field_label(attrs: attributes! { for="review-reason" }, "Why it is rejected (shown to the shopper, optional)")
                                input(attrs: attributes! { id="review-reason" name="reason" type="text" maxlength="300" })
                            )
                            <div>
                                button(
                                    variant: ButtonVariant::Destructive,
                                    attrs: attributes! { type="submit" },
                                    if review.is_published() { "Take down" } else { "Reject" }
                                )
                            </div>
                        </form>
                    </div>
                )
            }
        )
    })
}

/// The colour of where a review stands.
fn review_tone(status: ReviewStatus) -> Tone {
    match status {
        ReviewStatus::Pending => Tone::Warning,
        ReviewStatus::Published => Tone::Success,
        ReviewStatus::Rejected => Tone::Danger,
    }
}
