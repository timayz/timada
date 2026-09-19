//! `/{mount}/questions`: the product questions shoppers asked. A question
//! only reaches the storefront once it has an answer, so the default view is
//! the queue of those still waiting for one.

use std::collections::HashMap;

use serde::Deserialize;
use timada_catalog::products_by_ids;
use timada_customer::customers_by_ids;
use timada_review::{
    AnswerAuthor, ListQuestions, QuestionListRow, ReviewError, answers_of_questions,
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
        button::button,
        card::{card, card_content},
        textarea::textarea,
    },
    config::AdminServices,
    ui::{date, empty_state, page_header, pagination},
};

pub const PAGE_SIZE: u32 = 25;

#[query_params(error = bad_request)]
struct QuestionsQuery {
    page: Option<u32>,
    /// `waiting` (the default), `answered` or `all`.
    status: Option<String>,
    /// Set by [`reply`] when the answer was empty.
    error: Option<String>,
}

fn parse_answered(status: Option<&str>) -> Option<bool> {
    match status.unwrap_or("waiting") {
        "waiting" => Some(false),
        "answered" => Some(true),
        _ => None,
    }
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
    /// `(author, text, date)`.
    answers: Vec<(String, String, String)>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let query = query::<QuestionsQuery>(cx)?;
    let page = query.page.unwrap_or(1).max(1);
    let answered = parse_answered(query.status.as_deref());
    let selected = query.status.clone().unwrap_or_else(|| "waiting".to_owned());
    let empty_answer = query.error.as_deref() == Some("empty");
    let db = &app_context::<AdminServices>(cx).db;

    let rows = list_questions(
        db,
        &ListQuestions {
            answered,
            limit: PAGE_SIZE,
            offset: (page - 1) * PAGE_SIZE,
        },
    )
    .await?;
    let total = count_questions(db, answered).await?;

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
    let question_ids: Vec<String> = rows.iter().map(|r| r.question_id.clone()).collect();
    let mut answers: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for answer in answers_of_questions(db, &question_ids).await? {
        let author = match answer.author_customer_id {
            None => "Boutique".to_owned(),
            Some(customer_id) => format!("Client {customer_id}"),
        };
        answers.entry(answer.question_id).or_default().push((
            author,
            answer.body,
            date(answer.answered_at as u64),
        ));
    }
    let cards: Vec<QuestionCard> = rows
        .into_iter()
        .map(|row| question_card(cx, row, &products, &customers, &mut answers))
        .collect();

    Ok(view! {
        page_header(
            title: "Questions",
            <form method="get" class="flex items-center gap-2 text-sm">
                <label for="status" class="text-muted-foreground">"Statut"</label>
                <select id="status" name="status" class="h-9 rounded-lg border border-border bg-background px-3">
                    for (value, label) in [("waiting", "Sans réponse"), ("answered", "Répondues"), ("all", "Toutes")] {
                        <option value=(value) selected=(selected == value)>(label)</option>
                    }
                </select>
                <button type="submit" class="h-9 rounded-lg border border-border px-3">"Filtrer"</button>
            </form>
        )
        if empty_answer {
            <p role="alert" class="mb-4 text-sm text-destructive">"Écrivez une réponse avant de l'envoyer."</p>
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
    answers: &mut HashMap<String, Vec<(String, String, String)>>,
) -> QuestionCard {
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
        question_id: row.question_id,
        body: row.body,
    }
}

#[component]
async fn question_item(cx: &Cx, question: &QuestionCard) -> Result<impl View> {
    let answer_id = format!("answer-{}", question.question_id);
    let answer_label = format!("Réponse à la question sur {}", question.product_name);
    Ok(view! {
        card(card_content(
            <div class="flex flex-col gap-3 text-sm">
                <div class="flex flex-wrap items-center gap-2">
                    if question.answers.is_empty() {
                        badge(variant: BadgeVariant::Secondary, "Sans réponse")
                    } else {
                        badge(variant: BadgeVariant::Primary, "Répondue")
                    }
                    <span class="text-muted-foreground">(question.asked.clone())</span>
                </div>
                <p>
                    <a href=(question.product_link.clone()) class="underline-offset-4 hover:underline">(question.product_name.clone())</a>
                    <span class="text-muted-foreground">" · "</span>
                    <a href=(question.customer_link.clone()) class="text-muted-foreground underline-offset-4 hover:underline">(question.customer_label.clone())</a>
                </p>
                <p class="font-medium whitespace-pre-line">(question.body.clone())</p>
                for (author, text, answered) in &question.answers {
                    <p class="border-l-2 border-border pl-3">
                        <span class="text-muted-foreground">(author.clone()) " · " (answered.clone())</span>
                        <br>
                        <span class="whitespace-pre-line">(text.clone())</span>
                    </p>
                }
                <form method="post" action=(href!(reply).resolve(cx)) class="flex flex-col gap-2">
                    <input type="hidden" name="question_id" value=(question.question_id.clone())>
                    textarea(attrs: topcoat::view::attributes! { id=(answer_id) name="body" rows="2" required=(true) placeholder="Votre réponse, visible sur la fiche produit" aria-label=(answer_label) })
                    <div>
                        button(attrs: topcoat::view::attributes! { type="submit" }, "Répondre")
                    </div>
                </form>
            </div>
        ))
    })
}

#[derive(Debug, Deserialize)]
pub struct AnswerForm {
    question_id: String,
    body: String,
}

/// Answers as the shop's staff; the question then shows on the product page.
#[page(POST "./answer")]
pub async fn reply(cx: &Cx, Form(form): Form<AnswerForm>) -> Result<impl View> {
    let services = app_context::<AdminServices>(cx);
    let answered = timada_review::Command(&services.executor)
        .answer_question(
            &form.question_id,
            AnswerAuthor::Staff,
            form.body.trim().to_owned(),
        )
        .await;
    let target = match answered {
        Ok(()) => href!(index).resolve(cx),
        Err(ReviewError::Required(_)) => format!("{}?error=empty", href!(index).resolve(cx)),
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    Err::<(), _>(see_other(target).into())
}
