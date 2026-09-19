use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::QuestionAnswered,
    error::ReviewError,
    value_object::{AnswerAuthor, ModerationStatus},
};

impl<E: Executor> super::Command<'_, E> {
    /// Adds an answer that is published as it is written — the shop's own; a
    /// question may collect several. Answering a question that still awaits
    /// moderation publishes it; a refused question cannot be answered.
    /// Customers' answers go through [`Self::submit_answer`] instead.
    pub async fn answer_question(
        &self,
        id: impl Into<String>,
        author: AnswerAuthor,
        body: String,
    ) -> Result<(), ReviewError> {
        if body.trim().is_empty() {
            return Err(ReviewError::Required("body"));
        }
        let Some(question) = self.load_question(id).await? else {
            return Err(ReviewError::QuestionNotFound);
        };
        if question.status == ModerationStatus::Rejected {
            return Err(ReviewError::QuestionNotPublished);
        }

        question
            .write()?
            .event(&QuestionAnswered { author, body })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
