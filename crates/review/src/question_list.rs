//! SQL list read model of product questions and their answers: the
//! "Questions & réponses" of a product page and the admin's queue of
//! questions waiting for an answer. Fed by the `review-question-list`
//! subscription.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{QuestionAnswered, QuestionAsked},
    value_object::AnswerAuthor,
};

/// Subscription key; the caller attaches the pool with `.data(pool)`.
pub const QUESTION_LIST_SUBSCRIPTION: &str = "review-question-list";

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct QuestionListRow {
    pub question_id: String,
    pub product_id: String,
    pub customer_id: String,
    pub body: String,
    pub asked_at: i64,
    pub answer_count: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AnswerListRow {
    /// The id of the `QuestionAnswered` event: a question collects several.
    pub answer_id: String,
    pub question_id: String,
    /// `None` when the shop's staff answered.
    pub author_customer_id: Option<String>,
    pub body: String,
    pub answered_at: i64,
}

/// Filters for [`list_questions`], the admin queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListQuestions {
    /// `Some(false)` keeps the questions still waiting for an answer.
    pub answered: Option<bool>,
    pub limit: u32,
    pub offset: u32,
}

impl Default for ListQuestions {
    fn default() -> Self {
        Self {
            answered: None,
            limit: 50,
            offset: 0,
        }
    }
}

pub fn question_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(QUESTION_LIST_SUBSCRIPTION)
        .handler(insert_on_question_asked())
        .handler(insert_on_question_answered())
        .strict()
}

/// The answered questions of a product, newest first: a question only shows
/// on the product page once someone answered it.
pub async fn answered_questions(
    db: &SqlitePool,
    product_id: &str,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<QuestionListRow>> {
    sqlx::query_as(
        "SELECT question_id, product_id, customer_id, body, asked_at, answer_count
         FROM review_question_list
         WHERE product_id = ?1 AND answer_count > 0
         ORDER BY asked_at DESC, question_id DESC
         LIMIT ?2 OFFSET ?3",
    )
    .bind(product_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

/// How many answered questions a product has: the pages of [`answered_questions`].
pub async fn count_answered_questions(db: &SqlitePool, product_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_question_list WHERE product_id = ? AND answer_count > 0",
    )
    .bind(product_id)
    .fetch_one(db)
    .await
}

/// What a customer asked about a product and nobody answered yet — shown to
/// them alone.
pub async fn unanswered_questions_of(
    db: &SqlitePool,
    product_id: &str,
    customer_id: &str,
) -> sqlx::Result<Vec<QuestionListRow>> {
    sqlx::query_as(
        "SELECT question_id, product_id, customer_id, body, asked_at, answer_count
         FROM review_question_list
         WHERE product_id = ?1 AND customer_id = ?2 AND answer_count = 0
         ORDER BY asked_at DESC, question_id DESC",
    )
    .bind(product_id)
    .bind(customer_id)
    .fetch_all(db)
    .await
}

/// Questions across all products, oldest first: the queue is worked from the top.
pub async fn list_questions(
    db: &SqlitePool,
    filter: &ListQuestions,
) -> sqlx::Result<Vec<QuestionListRow>> {
    sqlx::query_as(
        "SELECT question_id, product_id, customer_id, body, asked_at, answer_count
         FROM review_question_list
         WHERE (?1 IS NULL OR (answer_count > 0) = ?1)
         ORDER BY asked_at, question_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(filter.answered)
    .bind(filter.limit)
    .bind(filter.offset)
    .fetch_all(db)
    .await
}

pub async fn count_questions(db: &SqlitePool, answered: Option<bool>) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_question_list WHERE (?1 IS NULL OR (answer_count > 0) = ?1)",
    )
    .bind(answered)
    .fetch_one(db)
    .await
}

/// The answers to the given questions, oldest first within each question.
pub async fn answers_of_questions(
    db: &SqlitePool,
    question_ids: &[String],
) -> sqlx::Result<Vec<AnswerListRow>> {
    if question_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT answer_id, question_id, author_customer_id, body, answered_at
         FROM review_answer_list
         WHERE question_id IN (",
    );
    let mut bound = query.separated(", ");
    for id in question_ids {
        bound.push_bind(id);
    }
    query.push(") ORDER BY answered_at, answer_id");
    query.build_query_as().fetch_all(db).await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

#[evento::subscription]
async fn insert_on_question_asked<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionAsked>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO review_question_list
            (question_id, product_id, customer_id, body, asked_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&event.aggregate_id)
    .bind(&event.data.product_id)
    .bind(&event.data.customer_id)
    .bind(&event.data.body)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

/// The answer is keyed by its event id and the count is recomputed from the
/// rows, so a redelivery changes nothing.
#[evento::subscription]
async fn insert_on_question_answered<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionAnswered>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    let author_customer_id = match &event.data.author {
        AnswerAuthor::Staff => None,
        AnswerAuthor::Customer { customer_id } => Some(customer_id.clone()),
    };
    sqlx::query(
        "INSERT OR IGNORE INTO review_answer_list
            (answer_id, question_id, author_customer_id, body, answered_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(event.id.to_string())
    .bind(&event.aggregate_id)
    .bind(author_customer_id)
    .bind(&event.data.body)
    .bind(event.timestamp as i64)
    .execute(&db)
    .await?;
    sqlx::query(
        "UPDATE review_question_list
         SET answer_count = (SELECT COUNT(*) FROM review_answer_list WHERE question_id = ?1)
         WHERE question_id = ?1",
    )
    .bind(&event.aggregate_id)
    .execute(&db)
    .await?;
    Ok(())
}
