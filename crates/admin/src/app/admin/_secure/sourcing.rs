//! `/{mount}/sourcing`: « À valider » — the prices the guardrails would not
//! let the shop set by itself.
//!
//! Approving one applies it. Refusing it locks the product's price, so the
//! same question is not put again in six hours and the queue can actually be
//! emptied; unlocking is how an operator asks to be told again. What was
//! never a price to begin with — a product with nothing to price, a rate that
//! could not be had — is filed instead.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::products_by_ids;
use timada_sourcing::{
    ListReviews, PriceReview, ReviewReason, SourcingError, count_reviews, list_reviews,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{content::Form, error::see_other, href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    app::admin::_secure::{
        products::product_id::{ProductId, show as show_product},
        suppliers::supplier_href,
    },
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonVariant, button},
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{date_time, empty_state, filter_bar, link, money, page_header, pagination, table_card},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct SourcingQuery {
    page: Option<u32>,
    /// `open` (the default) or `settled`.
    etat: Option<String>,
    error: Option<String>,
}

/// One line of the queue, with the product named rather than keyed.
struct Line {
    review: PriceReview,
    product_name: String,
    product_link: String,
    supplier_link: String,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<SourcingQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let settled = query.etat.as_deref() == Some("settled");
    let services = app_context::<AdminServices>(cx);
    let filter = ListReviews {
        settled: Some(settled),
        supplier_id: None,
        limit: PAGE_SIZE,
        offset: (page - 1) * PAGE_SIZE,
    };
    let reviews = list_reviews(&services.db, &filter).await?;
    let total = count_reviews(&services.db, &filter).await?;

    let product_ids: Vec<String> = reviews.iter().map(|r| r.product_id.clone()).collect();
    let names: HashMap<String, String> = products_by_ids(&services.db, &product_ids)
        .await?
        .into_iter()
        .map(|product| (product.id, product.name))
        .collect();
    let lines: Vec<Line> = reviews
        .into_iter()
        .map(|review| Line {
            product_name: names
                .get(&review.product_id)
                .cloned()
                .unwrap_or_else(|| review.product_id.clone()),
            product_link: href!(show_product, ProductId(review.product_id.clone())).resolve(cx),
            supplier_link: supplier_href(cx, &review.supplier_id),
            review,
        })
        .collect();

    Ok(view! {
        page_header(
            title: "Approvisionnement",
            filter_bar(
                <a href=(href!(index)) class=(tab_class(!settled))>"À valider"</a>
                <a href=(format!("{}?etat=settled", href!(index).resolve(cx))) class=(tab_class(settled))>"Réglés"</a>
            )
        )
        if let Some(error) = &query.error {
            <p class="mb-4 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm">
                (refusal(error))
            </p>
        }
        if lines.is_empty() {
            empty_state(message: if settled { "Rien de réglé pour l'instant." } else { "Rien à valider : les prix suivent leurs coûts." })
        } else {
            table_card(
                table(
                    table_header(table_row(
                        table_head("Produit") table_head("Motif")
                        table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Prix actuel")
                        table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Proposé")
                        table_head(attrs: topcoat::view::attributes! { class="text-right" }, "Coût")
                        table_head("Vu le") table_head("")
                    ))
                    table_body(for line in &lines { review_row(line: line, settled: settled) })
                )
            )
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

fn tab_class(active: bool) -> &'static str {
    if active {
        "rounded-md bg-muted px-3 py-1.5 text-sm font-medium"
    } else {
        "rounded-md px-3 py-1.5 text-sm text-muted-foreground hover:bg-muted"
    }
}

fn refusal(code: &str) -> &'static str {
    match code {
        "nothing" => "Il n'y a pas de prix à appliquer ici : voyez le produit.",
        "stale" => "Cette demande a déjà été réglée.",
        _ => "Action impossible.",
    }
}

#[topcoat::view::component]
async fn review_row(cx: &Cx, line: &Line, settled: bool) -> Result<impl View> {
    let review = &line.review;
    let moved = review.move_bp().map(|bp| {
        let sign = if bp >= 0 { "+" } else { "" };
        format!("{sign}{},{} %", bp / 100, (bp.abs() % 100) / 10)
    });
    Ok(view! {
        table_row(
            table_cell(
                link(href: line.product_link.clone(), (line.product_name.clone()))
                <a href=(line.supplier_link.clone()) class="ml-2 text-xs text-muted-foreground underline">"fournisseur"</a>
            )
            table_cell(
                badge(variant: if review.reason == ReviewReason::Floor { BadgeVariant::Destructive } else { BadgeVariant::Secondary }, (review.reason.label()))
            )
            table_cell(
                attrs: topcoat::view::attributes! { class="text-right tabular-nums" },
                if let Some(current) = &review.current { (money(current)) } else { "—" }
            )
            table_cell(
                attrs: topcoat::view::attributes! { class="text-right tabular-nums" },
                if review.reason.proposes_a_price() {
                    (money(&review.proposed))
                    if let Some(moved) = &moved {
                        <span class="ml-1 text-xs text-muted-foreground">(moved.clone())</span>
                    }
                } else { "—" }
            )
            table_cell(
                attrs: topcoat::view::attributes! { class="text-right tabular-nums" },
                (money(&review.cost))
                <span class="ml-1 text-xs text-muted-foreground">(format!("{} %", review.margin_bp / 100))</span>
            )
            table_cell((date_time(review.raised_at as u64)))
            table_cell(
                if settled {
                    if let Some(settled_as) = review.settled_as {
                        badge(variant: BadgeVariant::Outline, (settled_as.label()))
                    }
                } else {
                    <div class="flex justify-end gap-2">
                        if review.reason.proposes_a_price() {
                            <form method="post" action=(href!(approve).resolve(cx))>
                                <input type="hidden" name="review_id" value=(review.review_id.clone())>
                                button(attrs: topcoat::view::attributes! { type="submit" }, "Appliquer")
                            </form>
                            <form method="post" action=(href!(reject).resolve(cx))>
                                <input type="hidden" name="review_id" value=(review.review_id.clone())>
                                button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Refuser")
                            </form>
                        } else {
                            <form method="post" action=(href!(dismiss).resolve(cx))>
                                <input type="hidden" name="review_id" value=(review.review_id.clone())>
                                button(variant: ButtonVariant::Outline, attrs: topcoat::view::attributes! { type="submit" }, "Classer")
                            </form>
                        }
                    </div>
                }
            )
        )
    })
}

#[derive(Debug, Deserialize)]
pub struct ReviewForm {
    review_id: String,
}

#[page(POST "./approve")]
pub async fn approve(cx: &Cx, Form(form): Form<ReviewForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let done = timada_sourcing::Command::new(&services.executor, services.db.clone())
        .approve_price_change(&form.review_id)
        .await;
    Err::<(), _>(see_other(landing(cx, done)).into())
}

#[page(POST "./reject")]
pub async fn reject(cx: &Cx, Form(form): Form<ReviewForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let done = timada_sourcing::Command::new(&services.executor, services.db.clone())
        .reject_price_change(&form.review_id)
        .await;
    Err::<(), _>(see_other(landing(cx, done)).into())
}

#[page(POST "./dismiss")]
pub async fn dismiss(cx: &Cx, Form(form): Form<ReviewForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let done = timada_sourcing::Command::new(&services.executor, services.db.clone())
        .dismiss_review(&form.review_id)
        .await;
    Err::<(), _>(see_other(landing(cx, done)).into())
}

/// Back to the queue, saying why nothing happened when nothing did.
fn landing(cx: &Cx, done: std::result::Result<bool, SourcingError>) -> String {
    let path = href!(index).resolve(cx);
    match done {
        Ok(true) => path,
        Ok(false) => format!("{path}?error=stale"),
        Err(SourcingError::NothingToApply) => format!("{path}?error=nothing"),
        Err(err) => {
            tracing::error!(%err, "a price review could not be settled");
            format!("{path}?error=server")
        }
    }
}
