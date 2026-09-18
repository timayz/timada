mod answer_question;
mod ask_question;
mod publish_review;
mod reject_review;
mod submit_review;

use std::ops::Deref;

pub use ask_question::AskQuestion;
pub use submit_review::SubmitReview;

use evento::{Executor, Projection, metadata::Event};

use crate::{
    aggregator::{
        Question, QuestionAnswered, QuestionAsked, Review, ReviewPublished, ReviewRejected,
        ReviewSubmitted,
    },
    error::ReviewError,
    value_object::ReviewStatus,
};

/// Deterministic review id: one review per customer per product.
pub fn review_id(product_id: &str, customer_id: &str) -> String {
    timada_core::id::derived(&[product_id, customer_id], "review")
}

pub struct Command<'a, E: Executor>(pub &'a E);

impl<E: Executor> Deref for Command<'_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        self.0
    }
}

impl<E: Executor> Command<'_, E> {
    pub async fn load_review(&self, id: impl Into<String>) -> anyhow::Result<Option<ReviewState>> {
        review_projection().load(id).execute(self.0).await
    }

    pub async fn load_question(
        &self,
        id: impl Into<String>,
    ) -> anyhow::Result<Option<QuestionState>> {
        question_projection().load(id).execute(self.0).await
    }

    /// Loads a review that must exist and still await moderation.
    async fn load_pending(&self, id: impl Into<String>) -> Result<ReviewState, ReviewError> {
        let Some(review) = self.load_review(id).await? else {
            return Err(ReviewError::ReviewNotFound);
        };
        if review.status != ReviewStatus::Pending {
            return Err(ReviewError::NotPending);
        }
        Ok(review)
    }
}

/// Write-side state: just enough to guard the review commands.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct ReviewState {
    pub id: String,
    pub status: ReviewStatus,
}

// Strict + explicit skips: a non-strict projection only *reads* the events it
// handles, so the version it observes (and `write()` relies on) would go stale.
fn review_projection<E: Executor>() -> Projection<E, ReviewState> {
    Projection::new::<Review>()
        .handler(on_review_submitted())
        .handler(on_review_published())
        .handler(on_review_rejected())
        .strict()
}

#[evento::handler]
async fn on_review_submitted(
    event: Event<ReviewSubmitted>,
    row: &mut ReviewState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.status = ReviewStatus::Pending;
    Ok(())
}

#[evento::handler]
async fn on_review_published(
    _event: Event<ReviewPublished>,
    row: &mut ReviewState,
) -> anyhow::Result<()> {
    row.status = ReviewStatus::Published;
    Ok(())
}

#[evento::handler]
async fn on_review_rejected(
    _event: Event<ReviewRejected>,
    row: &mut ReviewState,
) -> anyhow::Result<()> {
    row.status = ReviewStatus::Rejected;
    Ok(())
}

/// Write-side state for questions.
#[evento::projection(id = id)]
#[evento::snapshot(none)]
pub struct QuestionState {
    pub id: String,
    pub product_id: String,
    pub answer_count: u32,
}

impl QuestionState {
    pub fn is_answered(&self) -> bool {
        self.answer_count > 0
    }
}

fn question_projection<E: Executor>() -> Projection<E, QuestionState> {
    Projection::new::<Question>()
        .handler(on_question_asked())
        .handler(on_question_answered())
        .strict()
}

#[evento::handler]
async fn on_question_asked(
    event: Event<QuestionAsked>,
    row: &mut QuestionState,
) -> anyhow::Result<()> {
    row.id = event.aggregate_id.to_owned();
    row.product_id = event.data.product_id;
    Ok(())
}

#[evento::handler]
async fn on_question_answered(
    _event: Event<QuestionAnswered>,
    row: &mut QuestionState,
) -> anyhow::Result<()> {
    row.answer_count += 1;
    Ok(())
}
