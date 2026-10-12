use evento::Executor;

use super::{Command, SessionState, UserState};

/// A presented session token that checks out, with the user behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSession {
    pub session: SessionState,
    pub user: UserState,
}

impl<E: Executor> Command<E> {
    /// The session and user behind a presented token, or `None` when the
    /// session is unknown, ended, expired at `now` (unix seconds), revoked by
    /// a "sign out everywhere", or its user is gone. This runs on every
    /// authenticated request.
    pub async fn resolve_session(
        &self,
        token_hash: &[u8; 32],
        now: u64,
    ) -> anyhow::Result<Option<ResolvedSession>> {
        let Some(session) = self.load_session(token_hash).await? else {
            return Ok(None);
        };
        if !session.is_valid_at(now) {
            return Ok(None);
        }
        let Some(user) = self.load_user(&session.user_id).await? else {
            return Ok(None);
        };
        if user
            .sessions_revoked
            .as_ref()
            .is_some_and(|revocation| revocation.covers(&session))
        {
            return Ok(None);
        }
        Ok(Some(ResolvedSession { session, user }))
    }
}
