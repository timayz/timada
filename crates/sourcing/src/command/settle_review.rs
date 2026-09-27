use evento::Executor;

use crate::{
    error::SourcingError,
    price_review::{PriceReview, Settled, review_by_id, settle_review},
};

use super::sourced_product_id;

impl<E: Executor> super::Command<'_, E> {
    /// Applies the price an operator looked at and agreed with.
    ///
    /// Answering a question somebody already answered does nothing and says
    /// so with `false`, so two operators clicking at once is harmless.
    pub async fn approve_price_change(&self, review_id: &str) -> Result<bool, SourcingError> {
        let review = self.open_review(review_id).await?;
        let Some(review) = review else {
            return Ok(false);
        };
        if !review.reason.proposes_a_price() {
            return Err(SourcingError::NothingToApply);
        }

        timada_pricing::Command(self.executor)
            .change_price(
                timada_pricing::price_id(&review.product_id),
                review.proposed.clone(),
            )
            .await?;
        let settled = settle_review(&self.db, review_id, Settled::Approved, self.now()?).await?;
        tracing::info!(
            product_id = %review.product_id,
            price = review.proposed.minor,
            "operator applied a price the guardrails held back"
        );
        Ok(settled)
    }

    /// Turns the price down — and settles the matter, by locking the price
    /// the product is on sale at. Without the lock the next pass would put
    /// the same question again within hours, and the queue would never
    /// empty; unlocking it is how an operator asks to be told again.
    pub async fn reject_price_change(&self, review_id: &str) -> Result<bool, SourcingError> {
        let review = self.open_review(review_id).await?;
        let Some(review) = review else {
            return Ok(false);
        };

        let settled = settle_review(&self.db, review_id, Settled::Rejected, self.now()?).await?;
        if settled {
            self.lock_source_price(
                sourced_product_id(&review.product_id),
                format!(
                    "changement refusé ({})",
                    review.reason.label().to_lowercase()
                ),
            )
            .await?;
        }
        Ok(settled)
    }

    /// Files a question that was never a price to begin with — a product
    /// with nothing to price, a rate that could not be had, a price in
    /// another currency the operator has now seen to. Nothing is applied and
    /// nothing is locked: the next pass may well ask again, and should.
    pub async fn dismiss_review(&self, review_id: &str) -> Result<bool, SourcingError> {
        if self.open_review(review_id).await?.is_none() {
            return Ok(false);
        }
        Ok(settle_review(&self.db, review_id, Settled::Stale, self.now()?).await?)
    }

    async fn open_review(&self, review_id: &str) -> Result<Option<PriceReview>, SourcingError> {
        Ok(review_by_id(&self.db, review_id)
            .await?
            .filter(PriceReview::is_open))
    }

    fn now(&self) -> Result<i64, SourcingError> {
        Ok(timada_core::time::now_unix_secs()? as i64)
    }
}
