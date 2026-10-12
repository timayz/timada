use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::Refreshed;
use crate::error::IdentityError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Extends a session the web layer has just re-issued (sliding
    /// expiration). `expires_at` is unix seconds. Returns whether a record
    /// was extended: `false` for an unknown or ended session, or when a
    /// concurrent request already extended it.
    pub async fn refresh_session(
        &self,
        token_hash: &[u8; 32],
        expires_at: u64,
        routing_key: Option<String>,
    ) -> Result<bool, IdentityError> {
        let Some(session) = self.load_session(token_hash).await? else {
            return Ok(false);
        };
        if session.ended {
            return Ok(false);
        }
        let refreshed = session
            .write()?
            .routing_key_opt(routing_key)
            .event(&Refreshed { expires_at })
            .commit(&self.0)
            .await;
        match refreshed {
            Ok(_) => Ok(true),
            // Two requests of the same browser raced; one extension is enough.
            Err(WriteError::InvalidOriginalVersion) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }
}
