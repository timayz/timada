//! `/{mount}/questions`: product questions and the answers customers give to
//! them. Nothing a customer writes is public before it went through here: the
//! default view is what awaits moderation — questions, and answers.
//! Answering a question as the shop publishes it.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::products_by_ids;
use timada_customer::customers_by_ids;
use timada_review::{
    AnswerAuthor, QuestionFilter, QuestionListRow, ReviewError, answers_of_questions,
    count_questions, list_questions,
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
        textarea::textarea,
    },
    config::AdminServices,
    ui::{date, empty_state, field, filter_bar, form_error, link, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct QuestionsQuery {
    page: Option<u32>,
    /// `review` (the default), `unanswered`, `answered`, `rejected` or `all`.
    status: Option<String>,
    /// Set by the actions when the review context refuses them.
    error: Option<String>,
}

fn parse_filter(status: Option<&str>) -> QuestionFilter {
    match status.unwrap_or("review") {
        "unanswered" => QuestionFilter::Unanswered,
        "answered" => QuestionFilter::Answered,
        "rejected" => QuestionFilter::Rejected,
        "all" => QuestionFilter::All,
        _ => QuestionFilter::ToReview,
    }
}

fn error_message(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "empty" => Some("Écrivez une réponse avant de l'envoyer."),
        "reason" => Some("Indiquez le motif du refus."),
        "stale" => Some("Déjà traité entre-temps : la file a été rechargée."),
        _ => None,
    }
}

/// An answer under a question of the queue.
struct AnswerItem {
    answer_id: String,
    author: String,
    written: String,
    body: String,
    pending: bool,
    rejection_reason: Option<String>,
}

/// One question of the queue, ready to render.
struct QuestionCard {
    question_id: String,
    product_name: String,
    product_link: String,
    customer_label: String,
    customer_link: String,
    asked: String,
    body: String,
    status_variant: BadgeVariant,
    status_label: &'static str,
    pending: bool,
    rejected: bool,
    rejection_reason: Option<String>,
    answers: Vec<AnswerItem>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<QuestionsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let filter = parse_filter(query.status.as_deref());
    let selected = query.status.clone().unwrap_or_else(|| "review".to_owned());
    let error = error_message(query.error.as_deref());
    let db = &app_context::<AdminServices>(cx).db;

    let rows = list_questions(db, filter, PAGE_SIZE, (page - 1) * PAGE_SIZE).await?;
    let total = count_questions(db, filter).await?;

    let product_ids: Vec<String> = rows.iter().map(|r| r.product_id.clone()).collect();
    let products: HashMap<String, String> = products_by_ids(db, &product_ids)
        .await?
        .into_iter()
        .map(|p| (p.id, p.name))
        .collect();
    let question_ids: Vec<String> = rows.iter().map(|r| r.question_id.clone()).collect();
    let answer_rows = answers_of_questions(db, &question_ids, false).await?;
    // Askers and answerers alike are shown by name.
    let mut customer_ids: Vec<String> = rows.iter().map(|r| r.customer_id.clone()).collect();
    customer_ids.extend(
        answer_rows
            .iter()
            .filter_map(|a| a.author_customer_id.clone()),
    );
    let customers: HashMap<String, String> = customers_by_ids(db, &customer_ids)
        .await?
        .into_iter()
        .map(|c| {
            let label = format!("{} {} · {}", c.first_name, c.last_name, c.email);
            (c.customer_id, label)
        })
        .collect();
    let mut answers: HashMap<String, Vec<AnswerItem>> = HashMap::new();
    for row in answer_rows {
        let author = match &row.author_customer_id {
            None => "Boutique".to_owned(),
            Some(id) => customers.get(id).cloned().unwrap_or_else(|| id.clone()),
        };
        answers
            .entry(row.question_id)
            .or_default()
            .push(AnswerItem {
                answer_id: row.answer_id,
                author,
                written: date(row.answered_at as u64),
                body: row.body,
                pending: row.status == "pending",
                rejection_reason: row.rejection_reason,
            });
    }
    let cards: Vec<QuestionCard> = rows
        .into_iter()
        .map(|row| question_card(cx, row, &products, &customers, &mut answers))
        .collect();

    Ok(view! {
        page_header(
            title: "Questions",
            filter_bar(
                field(
                    label: "Statut",
                    control: "status",
                    select(
                        attrs: topcoat::view::attributes! { id="status" name="status" },
                        for (value, label) in [("review", "À modérer"), ("unanswered", "Sans réponse"), ("answered", "Répondues"), ("rejected", "Refusées"), ("all", "Toutes")] {
                            <option value=(value) selected=(selected == value)>(label)</option>
                        }
                    )
                )
            )
        )
        if let Some(error) = error {
            form_error(class: "mb-4", (error))
        }
        if cards.is_empty() {
            empty_state(message: "Aucune question dans cette file.")
        } else {
            <div class="flex flex-col gap-4">
                for question in &cards { question_item(question: question) }
            </div>
            pagination(page: page, page_size: PAGE_SIZE, total: total as u64)
        }
    })
}

fn question_card(
    cx: &Cx,
    row: QuestionListRow,
    products: &HashMap<String, String>,
    customers: &HashMap<String, String>,
    answers: &mut HashMap<String, Vec<AnswerItem>>,
) -> QuestionCard {
    let (status_variant, status_label) = match (row.status.as_str(), row.answer_count) {
        ("pending", _) => (BadgeVariant::Secondary, "À modérer"),
        ("rejected", _) => (BadgeVariant::Destructive, "Refusée"),
        (_, 0) => (BadgeVariant::Outline, "Publiée, sans réponse"),
        _ => (BadgeVariant::Success, "Répondue"),
    };
    QuestionCard {
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
        asked: date(row.asked_at as u64),
        answers: answers.remove(&row.question_id).unwrap_or_default(),
        status_variant,
        status_label,
        pending: row.status == "pending",
        rejected: row.status == "rejected",
        rejection_reason: row.rejection_reason,
        question_id: row.question_id,
        body: row.body,
    }
}

#[component]
async fn question_item(cx: &Cx, question: &QuestionCard) -> Result<impl View> {
    let reply_id = format!("answer-{}", question.question_id);
    let reply_label = format!("Réponse à la question sur {}", question.product_name);
    let refusal_label = format!(
        "Motif du refus de la question sur {}",
        question.product_name
    );
    let reply_hint = if question.pending {
        "Votre réponse publie aussi la question"
    } else {
        "Votre réponse, visible sur la fiche produit"
    };
    Ok(view! {
        card(card_content(
            <div class="flex flex-col gap-3 text-sm">
                <div class="flex flex-wrap items-center gap-2">
                    badge(variant: question.status_variant, (question.status_label))
                    <span class="text-muted-foreground">(question.asked.clone())</span>
                </div>
                <p>
                    link(href: question.product_link.clone(), (question.product_name.clone()))
                    <span class="text-muted-foreground">" · "</span>
                    <a href=(question.customer_link.clone()) class="text-muted-foreground underline-offset-4 hover:underline">(question.customer_label.clone())</a>
                </p>
                <p class="font-medium whitespace-pre-line">(question.body.clone())</p>
                if let Some(reason) = &question.rejection_reason {
                    <p class="text-muted-foreground">"Motif du refus : " (reason.clone())</p>
                }
                for item in &question.answers {
                    answer_item(question_id: &question.question_id, item: item)
                }
                if question.pending {
                    <div class="flex flex-wrap items-center gap-3">
                        <form method="post" action=(href!(publish).resolve(cx))>
                            <input type="hidden" name="question_id" value=(question.question_id.clone())>
                            button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Publier la question")
                        </form>
                        <form method="post" action=(href!(refuse).resolve(cx)) class="flex items-center gap-2">
                            <input type="hidden" name="question_id" value=(question.question_id.clone())>
                            input(attrs: topcoat::view::attributes! { name="reason" required=(true) placeholder="Motif du refus" aria-label=(refusal_label) class="w-64" })
                            button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" }, "Refuser")
                        </form>
                    </div>
                }
                if !question.rejected {
                    <form method="post" action=(href!(reply).resolve(cx)) class="flex flex-col gap-2">
                        <input type="hidden" name="question_id" value=(question.question_id.clone())>
                        textarea(attrs: topcoat::view::attributes! { id=(reply_id) name="body" rows="2" required=(true) placeholder=(reply_hint) aria-label=(reply_label) })
                        <div>
                            button(attrs: topcoat::view::attributes! { type="submit" }, "Répondre")
                        </div>
                    </form>
                }
            </div>
        ))
    })
}

#[component]
async fn answer_item(cx: &Cx, question_id: &str, item: &AnswerItem) -> Result<impl View> {
    let refusal_label = format!("Motif du refus de la réponse de {}", item.author);
    Ok(view! {
        <div class="border-l-2 border-border pl-3">
            <p class="text-muted-foreground">
                (item.author.clone()) " · " (item.written.clone())
                if item.pending { " · " <strong>"à modérer"</strong> }
            </p>
            <p class="whitespace-pre-line">(item.body.clone())</p>
            if let Some(reason) = &item.rejection_reason {
                <p class="text-muted-foreground">"Refusée : " (reason.clone())</p>
            }
            if item.pending {
                <div class="mt-2 flex flex-wrap items-center gap-3">
                    <form method="post" action=(href!(publish_answer).resolve(cx))>
                        <input type="hidden" name="question_id" value=(question_id.to_owned())>
                        <input type="hidden" name="answer_id" value=(item.answer_id.clone())>
                        button(variant: ButtonVariant::Secondary, attrs: topcoat::view::attributes! { type="submit" }, "Publier la réponse")
                    </form>
                    <form method="post" action=(href!(refuse_answer).resolve(cx)) class="flex items-center gap-2">
                        <input type="hidden" name="question_id" value=(question_id.to_owned())>
                        <input type="hidden" name="answer_id" value=(item.answer_id.clone())>
                        input(attrs: topcoat::view::attributes! { name="reason" required=(true) placeholder="Motif du refus" aria-label=(refusal_label) class="w-64" })
                        button(variant: ButtonVariant::Destructive, attrs: topcoat::view::attributes! { type="submit" }, "Refuser")
                    </form>
                </div>
            }
        </div>
    })
}

/// Back to the queue; what the review context refuses becomes a message.
fn settled(cx: &Cx, outcome: std::result::Result<(), ReviewError>) -> Result<String> {
    let queue = href!(index).resolve(cx);
    let code = match outcome {
        Ok(()) => return Ok(queue),
        Err(ReviewError::Required("body")) => "empty",
        Err(ReviewError::Required(_)) => "reason",
        // Moderated by someone else in the meantime, or a double click.
        Err(
            ReviewError::QuestionNotPending
            | ReviewError::QuestionNotPublished
            | ReviewError::AnswerNotPending,
        ) => "stale",
        Err(err) => return Err(err.into()),
    };
    Ok(format!("{queue}?error={code}"))
}

#[derive(Debug, Deserialize)]
pub struct QuestionForm {
    question_id: String,
}

#[page(POST "./publish")]
pub async fn publish(cx: &Cx, Form(form): Form<QuestionForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_review::Command(&services.executor)
        .publish_question(&form.question_id)
        .await;
    Err::<(), _>(see_other(settled(cx, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct RefuseForm {
    question_id: String,
    reason: String,
}

#[page(POST "./refuse")]
pub async fn refuse(cx: &Cx, Form(form): Form<RefuseForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_review::Command(&services.executor)
        .reject_question(&form.question_id, form.reason)
        .await;
    Err::<(), _>(see_other(settled(cx, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct ReplyForm {
    question_id: String,
    body: String,
}

/// Answers as the shop's staff: published as written, and it publishes a
/// question that still awaited moderation.
#[page(POST "./answer")]
pub async fn reply(cx: &Cx, Form(form): Form<ReplyForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_review::Command(&services.executor)
        .answer_question(
            &form.question_id,
            AnswerAuthor::Staff,
            form.body.trim().to_owned(),
        )
        .await;
    Err::<(), _>(see_other(settled(cx, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct AnswerModerationForm {
    question_id: String,
    answer_id: String,
}

#[page(POST "./answers/publish")]
pub async fn publish_answer(cx: &Cx, Form(form): Form<AnswerModerationForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_review::Command(&services.executor)
        .publish_answer(&form.question_id, &form.answer_id)
        .await;
    Err::<(), _>(see_other(settled(cx, outcome)?).into())
}

#[derive(Debug, Deserialize)]
pub struct AnswerRefusalForm {
    question_id: String,
    answer_id: String,
    reason: String,
}

#[page(POST "./answers/refuse")]
pub async fn refuse_answer(cx: &Cx, Form(form): Form<AnswerRefusalForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let outcome = timada_review::Command(&services.executor)
        .reject_answer(&form.question_id, &form.answer_id, form.reason)
        .await;
    Err::<(), _>(see_other(settled(cx, outcome)?).into())
}
