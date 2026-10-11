use evento::Executor;

use super::{Command, UserState};

impl<E: Executor> Command<E> {
    /// The user behind a presented session token, or `None` when the session
    /// is unknown, ended, expired at `now` (unix seconds) or its user is gone.
    /// This runs on every authenticated request.
    pub async fn resolve_session(
        &self,
        token_hash: &[u8; 32],
        now: u64,
    ) -> anyhow::Result<Option<UserState>> {
        let Some(session) = self.load_session(token_hash).await? else {
            return Ok(None);
        };
        if !session.is_valid_at(now) {
            return Ok(None);
        }
        self.load_user(&session.user_id).await
    }
}
