use evento::{Executor, ProjectionAggregate, WriteError};

use crate::aggregator::{Registered, SessionEnded, SessionStarted, SessionsRevoked};
use crate::error::ShopperError;
use crate::value_object::{SESSION_TTL_SECS, generate_token, session_id, shopper_id};

use super::{SessionState, ShopperState, email_aad, normalize_email};
use timada_core::CodeVerifier;
use timada_core::keys::{self as key, Key as ShopperKey, Keys as ShopperKeys, new_key_id};

/// A sign-in that checked out. The token is the browser's to keep; the store
/// only knows its hash.
#[derive(Clone, PartialEq, Eq)]
pub struct SignedIn {
    pub token: String,
    pub shopper_id: String,
    /// Unix seconds.
    pub expires_at: u64,
}

impl std::fmt::Debug for SignedIn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SignedIn")
            .field("token", &"******")
            .field("shopper_id", &self.shopper_id)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// A presented token that checks out, with the shopper behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShopper {
    pub session: SessionState,
    pub shopper: ShopperState,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Burns the code mailed to `email`, creates the shopper on a first
    /// sign-in (with a fresh key, kept in `keys` first), and opens a session for [`SESSION_TTL_SECS`]. `now` is unix
    /// seconds.
    pub async fn sign_in(
        &self,
        keys: &dyn ShopperKeys,
        codes: &dyn CodeVerifier,
        email: &str,
        code: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<SignedIn, ShopperError> {
        let email = normalize_email(email)?;
        self.consume_code(codes, &email, code, now, routing_key.clone())
            .await?;

        let shopper_id = shopper_id(&email);
        let writer = match self.load_shopper_any(&shopper_id).await? {
            Some(shopper) if !shopper.forgotten => None,
            // The address deleted its account: it registers again, afresh.
            Some(forgotten) => Some(forgotten.write()?),
            None => Some(evento::append(&shopper_id).original_version(0).to_owned()),
        };
        if let Some(mut writer) = writer {
            // The key is kept before the event names it, so no event ever
            // points at a key nobody holds.
            let key_id = new_key_id();
            let key = ShopperKey::generate();
            let email = key::seal(&key, &email_aad(&shopper_id, &key_id), email.as_bytes())?;
            keys.put(&key_id, &key).await?;
            let registered = writer
                .routing_key_opt(routing_key.clone())
                .event(&Registered {
                    key_id: key_id.clone(),
                    email,
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            if registered.is_err() {
                // Nothing names the key: drop it rather than keep it unused.
                let _ = keys.delete(&key_id).await;
            }
            match registered {
                // A second browser registered the same address at once.
                Ok(_) | Err(WriteError::InvalidOriginalVersion) => {}
                Err(err) => return Err(err.into()),
            }
        }

        let token = generate_token();
        let expires_at = now + SESSION_TTL_SECS;
        let started = evento::append(session_id(&token))
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&SessionStarted {
                shopper_id: shopper_id.clone(),
                expires_at,
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        match started {
            Ok(_) => Ok(SignedIn {
                token,
                shopper_id,
                expires_at,
            }),
            // A 256-bit token collided: refuse rather than attach to a stranger.
            Err(WriteError::InvalidOriginalVersion) => {
                Err(anyhow::anyhow!("session token collision").into())
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Ends the session a token opened; unknown or ended is a no-op.
    pub async fn sign_out(
        &self,
        token: &str,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        let Some(session) = self.load_session(token).await? else {
            return Ok(());
        };
        if session.ended {
            return Ok(());
        }
        match session
            .write()?
            .routing_key_opt(routing_key)
            .event(&SessionEnded)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    /// Voids every session of the shopper started up to now, except the one
    /// `token` opened (the browser asking to sign out everywhere else).
    /// Sessions started afterwards are unaffected.
    pub async fn revoke_other_sessions(
        &self,
        shopper_id: &str,
        token: &str,
        routing_key: Option<String>,
    ) -> Result<(), ShopperError> {
        let shopper = self
            .load_shopper(shopper_id)
            .await?
            .ok_or(ShopperError::NotFound)?;
        shopper
            .write()?
            .routing_key_opt(routing_key)
            .event(&SessionsRevoked {
                except_session: Some(session_id(token)),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    /// The session and shopper behind a presented token, or `None` when the
    /// session is unknown, ended, revoked or expired at `now` (unix seconds),
    /// or its shopper deleted their account. This
    /// runs on every signed-in request.
    pub async fn resolve_session(
        &self,
        token: &str,
        now: u64,
    ) -> anyhow::Result<Option<ResolvedShopper>> {
        let Some(session) = self.load_session(token).await? else {
            return Ok(None);
        };
        if !session.is_valid_at(now) {
            return Ok(None);
        }
        let Some(shopper) = self.load_shopper(&session.shopper_id).await? else {
            return Ok(None);
        };
        if shopper
            .sessions_revoked
            .as_ref()
            .is_some_and(|revocation| revocation.covers(&session))
        {
            return Ok(None);
        }
        Ok(Some(ResolvedShopper { session, shopper }))
    }
}
