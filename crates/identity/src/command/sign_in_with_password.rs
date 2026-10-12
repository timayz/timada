use evento::Executor;

use crate::error::IdentityError;
use crate::value_object::{ChallengePurpose, LoginContext, SignInKind, normalize_email};

use super::sign_in::SignInFacts;

/// The second half of a password sign-in: the mailed code is back.
#[derive(Debug, Clone)]
pub struct SignInWithPassword {
    pub email: String,
    /// Who [`super::Command::check_password`] identified when the flow
    /// started; the address must still belong to them.
    pub user_id: String,
    pub code: String,
    /// The user the browser is signed in as, if any. Signing in as another
    /// user switches accounts.
    pub session_user_id: Option<String>,
    pub context: LoginContext,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordSignIn {
    pub user_id: String,
    /// Display name of that user, for the "signed in as" notice.
    pub label: String,
    /// The browser was signed in as this other user; the web layer replaces
    /// that session.
    pub switched_from: Option<String>,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    pub async fn sign_in_with_password(
        &self,
        cmd: SignInWithPassword,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<PasswordSignIn, IdentityError> {
        let email = normalize_email(&cmd.email)?;
        self.consume_email_code(
            &email,
            ChallengePurpose::SignIn,
            &cmd.code,
            now,
            routing_key.clone(),
        )
        .await?;
        match self.load_email_account(&email).await? {
            Some(owner) if owner.user_id == cmd.user_id => {}
            _ => return Err(IdentityError::InvalidState),
        }
        let user = self
            .record_sign_in(
                &cmd.user_id,
                SignInFacts {
                    kind: SignInKind::Password,
                    provider: None,
                    credential_id: None,
                    context: cmd.context,
                },
                routing_key,
            )
            .await?;
        Ok(PasswordSignIn {
            switched_from: cmd.session_user_id.filter(|me| *me != cmd.user_id),
            user_id: cmd.user_id,
            label: user.label,
        })
    }
}
