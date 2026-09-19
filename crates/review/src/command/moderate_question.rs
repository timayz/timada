//! Moderation of questions and of the answers customers give to them.
//! Nothing a customer writes is public before an operator let it through.

use evento::{Executor, ProjectionAggregate};

use crate::{
    aggregator::{
        AnswerPublished, AnswerRejected, AnswerSubmitted, QuestionPublished, QuestionRejected,
    },
    command::{QuestionState, answer_id},
    error::ReviewError,
    value_object::ModerationStatus,
};

impl<E: Executor> super::Command<'_, E> {
    async fn load_existing_question(
        &self,
        id: impl Into<String>,
    ) -> Result<QuestionState, ReviewError> {
        self.load_question(id)
            .await?
            .ok_or(ReviewError::QuestionNotFound)
    }

    /// Lets a question through. A no-op when it is already published.
    pub async fn publish_question(&self, id: impl Into<String>) -> Result<(), ReviewError> {
        let question = self.load_existing_question(id).await?;
        match question.status {
            ModerationStatus::Published => return Ok(()),
            ModerationStatus::Rejected => return Err(ReviewError::QuestionNotPending),
            ModerationStatus::Pending => {}
        }
        question
            .write()?
            .event(&QuestionPublished)
            .commit(self.0)
            .await?;
        tracing::info!(question_id = %question.id, "question published");
        Ok(())
    }

    /// Refuses a question that still awaits moderation.
    pub async fn reject_question(
        &self,
        id: impl Into<String>,
        reason: String,
    ) -> Result<(), ReviewError> {
        if reason.trim().is_empty() {
            return Err(ReviewError::Required("reason"));
        }
        let question = self.load_existing_question(id).await?;
        if question.status != ModerationStatus::Pending {
            return Err(ReviewError::QuestionNotPending);
        }
        question
            .write()?
            .event(&QuestionRejected {
                reason: reason.trim().to_owned(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(question_id = %question.id, "question rejected");
        Ok(())
    }

    /// A customer answers a published question; the answer awaits
    /// moderation. One answer per customer per question. Returns its id.
    pub async fn submit_answer(
        &self,
        question_id: impl Into<String>,
        customer_id: &str,
        body: String,
    ) -> Result<String, ReviewError> {
        if customer_id.trim().is_empty() {
            return Err(ReviewError::Required("customer_id"));
        }
        if body.trim().is_empty() {
            return Err(ReviewError::Required("body"));
        }
        let question = self.load_existing_question(question_id).await?;
        if question.status != ModerationStatus::Published {
            return Err(ReviewError::QuestionNotPublished);
        }
        let id = answer_id(&question.id, customer_id);
        if question.customer_answers.iter().any(|a| a.answer_id == id) {
            return Err(ReviewError::AlreadyAnswered);
        }
        question
            .write()?
            .event(&AnswerSubmitted {
                answer_id: id.clone(),
                customer_id: customer_id.to_owned(),
                body: body.trim().to_owned(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(question_id = %question.id, answer_id = %id, "answer submitted");
        Ok(id)
    }

    fn pending_answer(question: &QuestionState, answer_id: &str) -> Result<(), ReviewError> {
        match question
            .customer_answers
            .iter()
            .find(|a| a.answer_id == answer_id)
            .map(|a| a.status)
        {
            None => Err(ReviewError::AnswerNotFound),
            Some(ModerationStatus::Pending) => Ok(()),
            Some(_) => Err(ReviewError::AnswerNotPending),
        }
    }

    pub async fn publish_answer(
        &self,
        question_id: impl Into<String>,
        answer_id: &str,
    ) -> Result<(), ReviewError> {
        let question = self.load_existing_question(question_id).await?;
        Self::pending_answer(&question, answer_id)?;
        question
            .write()?
            .event(&AnswerPublished {
                answer_id: answer_id.to_owned(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(question_id = %question.id, %answer_id, "answer published");
        Ok(())
    }

    pub async fn reject_answer(
        &self,
        question_id: impl Into<String>,
        answer_id: &str,
        reason: String,
    ) -> Result<(), ReviewError> {
        if reason.trim().is_empty() {
            return Err(ReviewError::Required("reason"));
        }
        let question = self.load_existing_question(question_id).await?;
        Self::pending_answer(&question, answer_id)?;
        question
            .write()?
            .event(&AnswerRejected {
                answer_id: answer_id.to_owned(),
                reason: reason.trim().to_owned(),
            })
            .commit(self.0)
            .await?;
        tracing::info!(question_id = %question.id, %answer_id, "answer rejected");
        Ok(())
    }
}
