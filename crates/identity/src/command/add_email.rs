use evento::Executor;

use crate::aggregator::EmailAdded;
use crate::error::IdentityError;
use crate::value_object::{ChallengePurpose, normalize_email};

/// The e-mail + password method for a user who signed up with a provider or
/// a passkey, once the address was proven with a mailed code. The web layer
/// hashes the password when the form is submitted and carries the hash until
/// the code arrives.
#[derive(Clone)]
pub struct AddEmail {
    /// The user the browser is signed in as.
    pub session_user_id: Option<String>,
    /// The user the flow was started for; must be the signed-in one.
    pub bound_user_id: String,
    pub email: String,
    pub code: String,
    pub password_hash: String,
}

impl std::fmt::Debug for AddEmail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AddEmail")
            .field("session_user_id", &self.session_user_id)
            .field("bound_user_id", &self.bound_user_id)
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Burns the code, points the address at the user, then records the
    /// method on the user. The code is a [`ChallengePurpose::Register`] one:
    /// adding an address proves the same thing as registering one (a free
    /// address whose mail the person reads). A user who already has an
    /// address keeps it: the address of the method is set once. The index is
    /// claimed first so that a lost race never leaves a user with an address
    /// that belongs to someone else; a failure after the claim releases it
    /// again.
    pub async fn add_email(
        &self,
        cmd: AddEmail,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), IdentityError> {
        if cmd.session_user_id.as_deref() != Some(cmd.bound_user_id.as_str()) {
            return Err(IdentityError::SessionMismatch);
        }
        let user_id = cmd.bound_user_id;
        let email = normalize_email(&cmd.email)?;
        let Some(user) = self.load_user(&user_id).await? else {
            return Err(IdentityError::UserNotFound);
        };
        if user.password.is_some() {
            return Err(IdentityError::InvalidState);
        }
        self.consume_email_code(
            &email,
            ChallengePurpose::Register,
            &cmd.code,
            now,
            routing_key.clone(),
        )
        .await?;
        if let Some(owner) = self.load_email_account(&email).await?
            && self.load_user(&owner.user_id).await?.is_some()
        {
            return Err(IdentityError::AlreadyRegistered);
        }

        self.claim_email_index(&email, &user_id, routing_key.clone())
            .await?;
        let added = self
            .append_user(&user_id, routing_key.clone(), |user, builder| {
                if user.password.is_some() {
                    return Err(IdentityError::InvalidState);
                }
                builder.event(&EmailAdded {
                    email: email.clone(),
                    password_hash: cmd.password_hash.clone(),
                });
                Ok(())
            })
            .await;
        if let Err(err) = added {
            if let Err(release) = self
                .release_email_index(&user_id, &email, routing_key)
                .await
            {
                tracing::error!(%release, user_id, "cannot release the address after a failed add");
            }
            return Err(err);
        }
        Ok(())
    }
}
