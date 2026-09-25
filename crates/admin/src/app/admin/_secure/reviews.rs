//! `/{mount}/reviews`: the moderation queue. Reviews wait here until an
//! operator publishes or rejects them; only published ones reach the
//! storefront and the product rating.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::products_by_ids;
use timada_customer::customers_by_ids;
use timada_review::{
    ListReviews, ReviewError, ReviewListRow, ReviewStatus, count_reviews, list_reviews,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page, query_params, query_params as query},
    view::{View, component, view},
};

use super::{customers::customer_id, products::product_id};
use crate::{
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonVariant, button},
        card::{card, card_content},
        input::input,
        select::select,
    },
    config::AdminServices,
    ui::{date, empty_state, field, filter_bar, link, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct ReviewsQuery {
    page: Option<u32>,
    /// `pending` (the default), `published`, `rejected` or `all`.
    status: Option<String>,
}

fn parse_status(status: Option<&str>) -> Option<ReviewStatus> {
    match status.unwrap_or("pending") {
        "pending" => Some(ReviewStatus::Pending),
        "published" => Some(ReviewStatus::Published),
        "rejected" => Some(ReviewStatus::Rejected),
        _ => None,
    }
}

/// One review of the queue, ready to render.
struct ReviewCard {
    review_id: String,
    product_name: String,
    product_link: String,
    customer_label: String,
    customer_link: String,
    written: String,
    rating: String,
    title: String,
    body: String,
    verified: bool,
    pending: bool,
    status_variant: BadgeVariant,
    status_label: &'static str,
    rejection_reason: Option<String>,
}

fn status_badge(status: &str) -> (BadgeVariant, &'static str) {
    match status {
        "published" => (BadgeVariant::Success, "Publié"),
        "rejected" => (BadgeVariant::Destructive, "Refusé"),
        _ => (BadgeVariant::Secondary, "En attente"),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<ReviewsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let status = parse_status(query.status.as_deref());
    let selected = query.status.clone().unwrap_or_else(|| "pending".to_owned());
    let db = &app_context::<AdminServices>(cx).db;

    let rows = list_reviews(
        db,
        &ListReviews {
            status: status.clone(),
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_reviews(db, status.as_ref()).await?;

    let product_ids: Vec<String> = rows.iter().map(|r| r.product_id.clone()).collect();
    let products: HashMap<String, String> = products_by_ids(db, &product_ids)
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let customer_ids: Vec<String> = rows.iter().map(|r| r.customer_id.clone()).collect();
    let customers: HashMap<String, String> = customers_by_ids(db, &customer_ids)
        .await?
        .into_iter()
        .map(|c| {
            let label = format!("{} {} · {}", c.first_name, c.last_name, c.email);
            (c.customer_id, label)
        })
        .collect();
    let cards: Vec<ReviewCard> = rows
        .into_iter()
        .map(|row| review_card(cx, row, &products, &customers))
        .collect();

    Ok(view! {
        page_header(
            title: "Avis clients",
            filter_bar(
                field(
                    label: "Statut",
                    control: "status",
                    select(
                        attrs: topcoat::view::attributes! { id="status" name="status" },
                        for (value, label) in [("pending", "En attente"), ("published", "Publiés"), ("rejected", "Refusés"), ("all", "Tous")] {
                            <option value=(value) selected=(selected == value)>(label)</option>
                        }
                    )
                )
            )
        )
        if cards.is_empty() {
            empty_state(message: "Aucun avis dans cette file.")
        } else {
            <div class="flex flex-col gap-4">
                for review in &cards { review_item(review: review) }
            </div>
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

fn review_card(
    cx: &Cx,
    row: ReviewListRow,
    products: &HashMap<String, String>,
    customers: &HashMap<String, String>,
) -> ReviewCard {
    let (status_variant, status_label) = status_badge(&row.status);
    ReviewCard {
        product_name: products
            .get(&row.product_id)
            .cloned()
            .unwrap_or_else(|| row.product_id.clone()),
        product_link: href!(
            product_id::show,
            product_id::ProductId(row.product_id.clone())
        )
        .resolve(cx),
        customer_label: customers
            .get(&row.customer_id)
            .cloned()
            .unwrap_or_else(|| row.customer_id.clone()),
        customer_link: href!(
            customer_id::show,
            customer_id::CustomerId(row.customer_id.clone())
        )
        .resolve(cx),
        written: date(row.submitted_at as u64),
        rating: format!("{} / 5", row.rating),
        pending: row.status == ReviewStatus::Pending.as_str(),
        status_variant,
        status_label,
        review_id: row.review_id,
        title: row.title,
        body: row.body,
        verified: row.verified_purchase,
        rejection_reason: row.rejection_reason,
    }
}

#[component]
async fn review_item(cx: &Cx, review: &ReviewCard) -> Result<impl View> {
    let reason_id = format!("reason-{}", review.review_id);
    let reason_label = format!("Motif du refus de l'avis sur {}", review.product_name);
    Ok(view! {
        card(card_content(
            <div class="flex flex-col gap-3 text-sm">
                <div class="flex flex-wrap items-center gap-2">
                    <span class="font-semibold tabular-nums">(review.rating.clone())</span>
                    badge(variant: review.status_variant, (review.status_label))
                    if review.verified { badge(variant: BadgeVariant::Outline, "Achat vérifié") }
                    <span class="text-muted-foreground">(review.written.clone())</span>
                </div>
                <p>
                    link(href: review.product_link.clone(), (review.product_name.clone()))
                    <span class="text-muted-foreground">" · "</span>
                    <a href=(review.customer_link.clone()) class="text-muted-foreground underline-offset-4 hover:underline">(review.customer_label.clone())</a>
                </p>
                if !review.title.is_empty() { <p class="font-medium">(review.title.clone())</p> }
                <p class="whitespace-pre-line">(review.body.clone())</p>
                if let Some(reason) = &review.rejection_reason {
                    <p class="text-muted-foreground">"Motif du refus : " (reason.clone())</p>
                }
                if review.pending {
                    <div class="flex flex-wrap items-center gap-3">
                        <form method="post" action=(href!(publish).resolve(cx))>
                            <input type="hidden" name="review_id" value=(review.review_id.clone())>
                            button(attrs: topcoat::view::attributes! { type="submit" }, "Publier")
                        </form>
                        <form method="post" action=(href!(reject).resolve(cx)) class="flex items-center gap-2">
                            <input type="hidden" name="review_id" value=(review.review_id.clone())>
                            input(attrs: topcoat::view::attributes! { id=(reason_id) name="reason" required=(true) placeholder="Motif du refus" aria-label=(reason_label) class="w-64" })
                            button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" }, "Refuser")
                        </form>
                    </div>
                }
            </div>
        ))
    })
}

#[derive(Debug, Deserialize)]
pub struct PublishForm {
    review_id: String,
}

/// A review someone else already moderated is left as it is.
fn settled(result: std::result::Result<(), ReviewError>) -> Result<()> {
    match result {
        Ok(()) | Err(ReviewError::NotPending) => Ok(()),
        Err(err) => Err(anyhow::Error::from(err).into()),
    }
}

#[page(POST "./publish")]
pub async fn publish(cx: &Cx, Form(form): Form<PublishForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    settled(
        timada_review::Command(&services.executor)
            .publish_review(&form.review_id)
            .await,
    )?;
    Err::<(), _>(see_other(href!(index).resolve(cx)).into())
}

#[derive(Debug, Deserialize)]
pub struct RejectForm {
    review_id: String,
    reason: String,
}

#[page(POST "./reject")]
pub async fn reject(cx: &Cx, Form(form): Form<RejectForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    settled(
        timada_review::Command(&services.executor)
            .reject_review(&form.review_id, form.reason.trim().to_owned())
            .await,
    )?;
    Err::<(), _>(see_other(href!(index).resolve(cx)).into())
}
