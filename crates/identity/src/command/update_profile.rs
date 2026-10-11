use evento::Executor;

use crate::aggregator::ProfileUpdated;
use crate::error::IdentityError;

#[evento::command]
impl<E: Executor> super::Command<E> {
    pub async fn update_profile(
        &self,
        user_id: &str,
        account: &str,
        label: &str,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        let account = account.trim();
        let label = label.trim();
        if account.is_empty() || label.is_empty() || account.len() > 255 || label.len() > 255 {
            return Err(IdentityError::InvalidState);
        }
        self.append_user(user_id, routing_key, |user, builder| {
            if user.account == account && user.label == label {
                return Err(IdentityError::InvalidState);
            }
            builder.event(&ProfileUpdated {
                account: account.to_owned(),
                label: label.to_owned(),
            });
            Ok(())
        })
        .await
        .map(|_| ())
        .or_else(|err| match err {
            IdentityError::InvalidState => Ok(()),
            other => Err(other),
        })
    }
}
