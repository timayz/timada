use evento::Executor;

use crate::aggregator::SignedIn;
use crate::error::IdentityError;
use crate::value_object::{LoginContext, SignInMethod};

use super::{Command, UserState};

/// What a successful sign-in records.
#[derive(Debug, Clone)]
pub(crate) struct SignInFacts {
    pub method: SignInMethod,
    pub provider: Option<String>,
    pub credential_id: Option<String>,
    pub context: LoginContext,
}

impl<E: Executor> Command<E> {
    /// Records a sign-in on the user and repairs a missing first-user admin
    /// grant. The session itself is started by the caller once it holds a
    /// token hash.
    pub(crate) async fn record_sign_in(
        &self,
        user_id: &str,
        facts: SignInFacts,
        routing_key: Option<String>,
    ) -> Result<UserState, IdentityError> {
        let user = self
            .append_user(user_id, routing_key.clone(), |_, builder| {
                builder.event(&SignedIn {
                    method: facts.method,
                    provider: facts.provider.clone(),
                    credential_id: facts.credential_id.clone(),
                    user_agent: facts.context.user_agent.clone(),
                    ip: facts.context.ip.clone(),
                });
                Ok(())
            })
            .await?;
        self.grant_admin_if_first(&user, routing_key).await?;
        Ok(user)
    }
}
