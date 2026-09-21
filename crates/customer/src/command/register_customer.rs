use evento::{Executor, ProjectionAggregate};
use timada_core::Civility;

use crate::{
    aggregator::{CustomerAccountOpened, CustomerRegistered, CustomerRegisteredAsGuest},
    error::CustomerError,
};

#[derive(Debug, Clone)]
pub struct RegisterCustomer {
    pub email: String,
    pub civility: Civility,
    pub first_name: String,
    pub last_name: String,
}

#[evento::command]
impl<E: Executor> super::Command<'_, E> {
    /// Creates a customer account. Credentials are handled elsewhere.
    pub async fn register_customer(
        &self,
        cmd: RegisterCustomer,
        routing_key: Option<String>,
    ) -> Result<String, CustomerError> {
        let registered = registration(cmd)?;
        let id = evento::create()
            .routing_key_opt(routing_key)
            .event(&registered)
            .commit(self.0)
            .await?;
        tracing::info!(customer_id = %id, "customer registered");
        Ok(id)
    }

    /// Registers somebody who orders without an account: a customer to
    /// deliver, invoice and write to, with nothing to sign in to. The e-mail
    /// may be one an account already uses — a guest claims nothing.
    pub async fn register_guest(
        &self,
        cmd: RegisterCustomer,
        routing_key: Option<String>,
    ) -> Result<String, CustomerError> {
        let registered = registration(cmd)?;
        let id = evento::create()
            .routing_key_opt(routing_key)
            .event(&registered)
            .event(&CustomerRegisteredAsGuest)
            .commit(self.0)
            .await?;
        tracing::info!(customer_id = %id, "guest registered");
        Ok(id)
    }

    /// The guest has an account from now on. `false` when the customer
    /// already had one — asking twice is harmless.
    pub async fn open_account(&self, id: impl Into<String>) -> Result<bool, CustomerError> {
        let customer = self.load_existing(id).await?;
        if !customer.guest {
            return Ok(false);
        }
        customer
            .write()?
            .event(&CustomerAccountOpened)
            .commit(self.0)
            .await?;
        tracing::info!(customer_id = %customer.id, "guest opened an account");
        Ok(true)
    }
}

fn registration(cmd: RegisterCustomer) -> Result<CustomerRegistered, CustomerError> {
    let email = validate_email(&cmd.email)?;
    if cmd.first_name.trim().is_empty() {
        return Err(CustomerError::Required("first_name"));
    }
    if cmd.last_name.trim().is_empty() {
        return Err(CustomerError::Required("last_name"));
    }
    Ok(CustomerRegistered {
        email,
        civility: cmd.civility,
        first_name: cmd.first_name,
        last_name: cmd.last_name,
    })
}

/// Normalises and minimally validates an email address.
pub(super) fn validate_email(email: &str) -> Result<String, CustomerError> {
    let email = email.trim().to_lowercase();
    let well_formed = email
        .split_once('@')
        .is_some_and(|(local, domain)| !local.is_empty() && domain.contains('.'));
    if well_formed {
        Ok(email)
    } else {
        Err(CustomerError::InvalidEmail(email))
    }
}
