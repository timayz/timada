use evento::Executor;
use timada_core::Civility;

use crate::{aggregator::CustomerRegistered, error::CustomerError};

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
        let email = validate_email(&cmd.email)?;
        if cmd.first_name.trim().is_empty() {
            return Err(CustomerError::Required("first_name"));
        }
        if cmd.last_name.trim().is_empty() {
            return Err(CustomerError::Required("last_name"));
        }

        let id = evento::create()
            .routing_key_opt(routing_key)
            .event(&CustomerRegistered {
                email,
                civility: cmd.civility,
                first_name: cmd.first_name,
                last_name: cmd.last_name,
            })
            .commit(self.0)
            .await?;
        tracing::info!(customer_id = %id, "customer registered");
        Ok(id)
    }
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
