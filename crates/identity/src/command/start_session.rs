use evento::{Executor, WriteError};

use crate::aggregator::Started;
use crate::error::IdentityError;
use crate::value_object::{LoginContext, session_id};

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records a session the web layer has just issued. `expires_at` is unix
    /// seconds.
    pub async fn start_session(
        &self,
        token_hash: &[u8; 32],
        user_id: &str,
        expires_at: u64,
        context: LoginContext,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if self.load_user(user_id).await?.is_none() {
            return Err(IdentityError::UserNotFound);
        }
        let started = evento::append(session_id(token_hash))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Started {
                user_id: user_id.to_owned(),
                expires_at,
                user_agent: context.user_agent,
                ip: context.ip,
            })
            .commit(&self.executor)
            .await;
        match started {
            Ok(_) => Ok(()),
            // A 256-bit token collided: refuse rather than attach to a stranger.
            Err(WriteError::InvalidOriginalVersion) => Err(IdentityError::InvalidState),
            Err(err) => Err(err.into()),
        }
    }
}
