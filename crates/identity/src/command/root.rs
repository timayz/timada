use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{RootEstablished, SignedInWithPassword};
use crate::error::IdentityError;
use crate::root::RootAccount;
use crate::value_object::{LoginContext, ROOT_USER_ID};

use super::{PasswordSignIn, USER_APPEND_ATTEMPTS};

/// The login form filled in with what may be root's address and password.
#[derive(Clone)]
pub struct SignInRoot {
    pub email: String,
    pub password: String,
    /// The user the browser is signed in as, if any. Signing in as root
    /// from another account switches accounts.
    pub session_user_id: Option<String>,
    pub context: LoginContext,
}

impl std::fmt::Debug for SignInRoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignInRoot")
            .field("email", &self.email)
            .finish_non_exhaustive()
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Signs root in: the pair is checked against the configuration's
    /// (`InvalidCredentials` otherwise), with no mailed code, so root gets in
    /// while no mail leaves. Root's stream is written the first time, and
    /// follows the configured address when that changed; the password never
    /// enters the store.
    pub async fn sign_in_root(
        &self,
        root: &RootAccount,
        cmd: SignInRoot,
        routing_key: Option<String>,
    ) -> Result<PasswordSignIn, IdentityError> {
        if !root.check(&cmd.email, &cmd.password).await? {
            return Err(IdentityError::InvalidCredentials);
        }
        let established = RootEstablished {
            email: root.email().to_owned(),
        };
        let signed_in = SignedInWithPassword {
            user_agent: cmd.context.user_agent,
            ip: cmd.context.ip,
        };
        let mut attempt = 0;
        loop {
            attempt += 1;
            let written = match self.load_user_any(ROOT_USER_ID).await? {
                Some(user) => {
                    let mut builder = user.write()?;
                    builder.routing_key_opt(routing_key.clone());
                    if user.account != root.email() {
                        builder.event(&established);
                    }
                    builder.event(&signed_in).commit(&self.executor).await
                }
                None => {
                    evento::append(ROOT_USER_ID)
                        .original_version(0)
                        .routing_key_opt(routing_key.clone())
                        .event(&established)
                        .event(&signed_in)
                        .commit(&self.executor)
                        .await
                }
            };
            match written {
                Ok(_) => break,
                // Root signing in from two browsers at once: again, on the
                // stream as the other left it.
                Err(WriteError::InvalidOriginalVersion) if attempt < USER_APPEND_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok(PasswordSignIn {
            switched_from: cmd.session_user_id.filter(|me| me != ROOT_USER_ID),
            user_id: ROOT_USER_ID.to_owned(),
            label: root.email().to_owned(),
        })
    }
}
