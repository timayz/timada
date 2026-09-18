use evento::{Executor, ProjectionAggregate};

use crate::{aggregator::CustomerEmailChanged, error::CustomerError};

use super::register_customer::validate_email;

impl<E: Executor> super::Command<'_, E> {
    pub async fn change_email(
        &self,
        id: impl Into<String>,
        email: String,
    ) -> Result<(), CustomerError> {
        let email = validate_email(&email)?;
        let customer = self.load_existing(id).await?;

        customer
            .write()?
            .event(&CustomerEmailChanged { email })
            .commit(self.0)
            .await?;
        Ok(())
    }
}
