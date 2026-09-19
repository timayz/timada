//! SQL list read model of product questions and their answers: the
//! "Questions & réponses" of a product page and the admin's moderation queue.
//! Fed by the `review-question-list` subscription.
//!
//! Only what moderation let through is public: a published question with its
//! published answers. The shop's own answers are published as they are
//! written, and answering a pending question publishes it.

use evento::{
    Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use sqlx::SqlitePool;

use crate::{
    aggregator::{
        AnswerPublished, AnswerRejected, AnswerSubmitted, QuestionAnswered, QuestionAsked,
        QuestionPublished, QuestionRejected,
    },
    value_object::{AnswerAuthor, ModerationStatus},
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
    /// Published answers.
    pub answer_count: i64,
    /// [`ModerationStatus::as_str`].
    pub status: String,
    pub rejection_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct AnswerListRow {
    /// The `QuestionAnswered` event's id for the shop's answers, the derived
    /// [`crate::answer_id`] for a customer's.
    pub answer_id: String,
    pub question_id: String,
    /// `None` when the shop's staff answered.
    pub author_customer_id: Option<String>,
    pub body: String,
    pub answered_at: i64,
    /// [`ModerationStatus::as_str`].
    pub status: String,
    pub rejection_reason: Option<String>,
}

/// Which questions the admin queue shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum QuestionFilter {
    /// A question, or an answer to it, awaits moderation.
    #[default]
    ToReview,
    /// Published, and nobody answered yet.
    Unanswered,
    /// Published, with at least one published answer.
    Answered,
    Rejected,
    All,
}

impl QuestionFilter {
    fn selector(self) -> i64 {
        match self {
            QuestionFilter::ToReview => 0,
            QuestionFilter::Unanswered => 1,
            QuestionFilter::Answered => 2,
            QuestionFilter::Rejected => 3,
            QuestionFilter::All => 4,
        }
    }
}

pub fn question_list_subscription<E: Executor>() -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(QUESTION_LIST_SUBSCRIPTION)
        .handler(insert_on_question_asked())
        .handler(insert_on_question_answered())
        .handler(status_on_question_published())
        .handler(status_on_question_rejected())
        .handler(insert_on_answer_submitted())
        .handler(status_on_answer_published())
        .handler(status_on_answer_rejected())
        .strict()
}

/// The published questions of a product, newest first.
pub async fn published_questions(
    db: &SqlitePool,
    product_id: &str,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<QuestionListRow>> {
    sqlx::query_as(
        "SELECT question_id, product_id, customer_id, body, asked_at, answer_count, status,
                rejection_reason
         FROM review_question_list
         WHERE product_id = ?1 AND status = 'published'
         ORDER BY asked_at DESC, question_id DESC
         LIMIT ?2 OFFSET ?3",
    )
    .bind(product_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

/// How many published questions a product has: the pages of [`published_questions`].
pub async fn count_published_questions(db: &SqlitePool, product_id: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_question_list WHERE product_id = ? AND status = 'published'",
    )
    .bind(product_id)
    .fetch_one(db)
    .await
}

/// What a customer asked about a product that is not public: still awaiting
/// moderation, or refused — shown to them alone.
pub async fn own_unpublished_questions(
    db: &SqlitePool,
    product_id: &str,
    customer_id: &str,
) -> sqlx::Result<Vec<QuestionListRow>> {
    sqlx::query_as(
        "SELECT question_id, product_id, customer_id, body, asked_at, answer_count, status,
                rejection_reason
         FROM review_question_list
         WHERE product_id = ?1 AND customer_id = ?2 AND status != 'published'
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
    filter: QuestionFilter,
    limit: u32,
    offset: u32,
) -> sqlx::Result<Vec<QuestionListRow>> {
    sqlx::query_as(
        "SELECT q.question_id, q.product_id, q.customer_id, q.body, q.asked_at, q.answer_count,
                q.status, q.rejection_reason
         FROM review_question_list q
         WHERE ?1 = 4
            OR (?1 = 0 AND (q.status = 'pending' OR EXISTS (
                    SELECT 1 FROM review_answer_list a
                    WHERE a.question_id = q.question_id AND a.status = 'pending')))
            OR (?1 = 1 AND q.status = 'published' AND q.answer_count = 0)
            OR (?1 = 2 AND q.status = 'published' AND q.answer_count > 0)
            OR (?1 = 3 AND q.status = 'rejected')
         ORDER BY q.asked_at, q.question_id
         LIMIT ?2 OFFSET ?3",
    )
    .bind(filter.selector())
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

pub async fn count_questions(db: &SqlitePool, filter: QuestionFilter) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM review_question_list q
         WHERE ?1 = 4
            OR (?1 = 0 AND (q.status = 'pending' OR EXISTS (
                    SELECT 1 FROM review_answer_list a
                    WHERE a.question_id = q.question_id AND a.status = 'pending')))
            OR (?1 = 1 AND q.status = 'published' AND q.answer_count = 0)
            OR (?1 = 2 AND q.status = 'published' AND q.answer_count > 0)
            OR (?1 = 3 AND q.status = 'rejected')",
    )
    .bind(filter.selector())
    .fetch_one(db)
    .await
}

/// The answers to the given questions, oldest first within each question.
/// `only_published` is what a storefront passes; the admin wants them all.
pub async fn answers_of_questions(
    db: &SqlitePool,
    question_ids: &[String],
    only_published: bool,
) -> sqlx::Result<Vec<AnswerListRow>> {
    if question_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut query = sqlx::QueryBuilder::<sqlx::Sqlite>::new(
        "SELECT answer_id, question_id, author_customer_id, body, answered_at, status,
                rejection_reason
         FROM review_answer_list
         WHERE question_id IN (",
    );
    let mut bound = query.separated(", ");
    for id in question_ids {
        bound.push_bind(id);
    }
    query.push(")");
    if only_published {
        query.push(" AND status = 'published'");
    }
    query.push(" ORDER BY answered_at, answer_id");
    query.build_query_as().fetch_all(db).await
}

fn pool<E: Executor>(ctx: &Context<'_, E>) -> anyhow::Result<SqlitePool> {
    ctx.get::<SqlitePool>()
        .ok_or_else(|| anyhow::anyhow!("SqlitePool missing from subscription context"))
}

/// The published answers are counted from the rows, so a redelivery changes nothing.
async fn recount(db: &SqlitePool, question_id: &str) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE review_question_list
         SET answer_count = (SELECT COUNT(*) FROM review_answer_list
                             WHERE question_id = ?1 AND status = 'published')
         WHERE question_id = ?1",
    )
    .bind(question_id)
    .execute(db)
    .await?;
    Ok(())
}

async fn set_question_status(
    db: &SqlitePool,
    question_id: &str,
    status: ModerationStatus,
    reason: Option<&str>,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE review_question_list SET status = ?, rejection_reason = ? WHERE question_id = ?",
    )
    .bind(status.as_str())
    .bind(reason)
    .bind(question_id)
    .execute(db)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn insert_on_question_asked<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionAsked>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO review_question_list
            (question_id, product_id, customer_id, body, asked_at, status)
         VALUES (?, ?, ?, ?, ?, 'pending')",
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

/// An answer published as written, keyed by its event id. The shop answering
/// a pending question publishes it.
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
    let by_staff = author_customer_id.is_none();
    sqlx::query(
        "INSERT OR IGNORE INTO review_answer_list
            (answer_id, question_id, author_customer_id, body, answered_at, status)
         VALUES (?, ?, ?, ?, ?, 'published')",
    )
    .bind(event.id.to_string())
    .bind(&event.aggregate_id)
    .bind(author_customer_id)
    .bind(&event.data.body)
    .bind(event.timestamp as i64)
    .execute(&db)
    .await?;
    if by_staff {
        sqlx::query(
            "UPDATE review_question_list SET status = 'published'
             WHERE question_id = ? AND status = 'pending'",
        )
        .bind(&event.aggregate_id)
        .execute(&db)
        .await?;
    }
    recount(&db, &event.aggregate_id).await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_question_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionPublished>,
) -> anyhow::Result<()> {
    set_question_status(
        &pool(ctx)?,
        &event.aggregate_id,
        ModerationStatus::Published,
        None,
    )
    .await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_question_rejected<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<QuestionRejected>,
) -> anyhow::Result<()> {
    set_question_status(
        &pool(ctx)?,
        &event.aggregate_id,
        ModerationStatus::Rejected,
        Some(&event.data.reason),
    )
    .await?;
    Ok(())
}

#[evento::subscription]
async fn insert_on_answer_submitted<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<AnswerSubmitted>,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO review_answer_list
            (answer_id, question_id, author_customer_id, body, answered_at, status)
         VALUES (?, ?, ?, ?, ?, 'pending')",
    )
    .bind(&event.data.answer_id)
    .bind(&event.aggregate_id)
    .bind(&event.data.customer_id)
    .bind(&event.data.body)
    .bind(event.timestamp as i64)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_answer_published<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<AnswerPublished>,
) -> anyhow::Result<()> {
    let db = pool(ctx)?;
    sqlx::query("UPDATE review_answer_list SET status = 'published' WHERE answer_id = ?")
        .bind(&event.data.answer_id)
        .execute(&db)
        .await?;
    recount(&db, &event.aggregate_id).await?;
    Ok(())
}

#[evento::subscription]
async fn status_on_answer_rejected<E: Executor>(
    ctx: &Context<'_, E>,
    event: Event<AnswerRejected>,
) -> anyhow::Result<()> {
    sqlx::query(
        "UPDATE review_answer_list SET status = 'rejected', rejection_reason = ?
         WHERE answer_id = ?",
    )
    .bind(&event.data.reason)
    .bind(&event.data.answer_id)
    .execute(&pool(ctx)?)
    .await?;
    Ok(())
}
