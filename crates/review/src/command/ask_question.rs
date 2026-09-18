use evento::Executor;

use crate::{aggregator::QuestionAsked, error::ReviewError};

#[derive(Debug, Clone)]
pub struct AskQuestion {
    pub product_id: String,
    pub customer_id: String,
    pub body: String,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    pub async fn ask_question(
        &self,
        cmd: AskQuestion,
        routing_key: Option<String>,
    ) -> Result<String, ReviewError> {
        if cmd.product_id.trim().is_empty() {
            return Err(ReviewError::Required("product_id"));
        }
        if cmd.customer_id.trim().is_empty() {
            return Err(ReviewError::Required("customer_id"));
        }
        if cmd.body.trim().is_empty() {
            return Err(ReviewError::Required("body"));
        }

        let id = evento::create()
            .routing_key_opt(routing_key)
            .event(&QuestionAsked {
                product_id: cmd.product_id,
                customer_id: cmd.customer_id,
                body: cmd.body,
            })
            .commit(self.0)
            .await?;
        tracing::info!(question_id = %id, "question asked");
        Ok(id)
    }
}
