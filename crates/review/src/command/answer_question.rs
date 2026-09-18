use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::QuestionAnswered, error::ReviewError, value_object::AnswerAuthor};

impl<E: Executor> super::Command<'_, E> {
    /// Adds an answer; a question may collect several.
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

        question
            .write()?
            .event(&QuestionAnswered { author, body })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
