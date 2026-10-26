use evento::{Executor, ProjectionAggregate, WriteBuilder, WriteError};
use timada_core::keys::{self, Key, Keys, new_key_id};

use crate::aggregator::{Confirmed, Requested, Unsubscribed};
use crate::error::NewsletterError;
use crate::value_object::{
    CONFIRM_TTL_SECS, ConfirmLink, RESEND_AFTER_SECS, SOURCE_ACCOUNT, SubscriberStatus,
    UnsubscribeLink, check_source, confirm_token, normalize_email, subscriber_id,
};

use super::{SubscriberState, email_aad};

/// What a shopper asks for: the shop's news at `email`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestSubscription {
    pub email: String,
    /// A language tag or a whole `Accept-Language` value; only the primary
    /// subtag is kept.
    pub language: Option<String>,
    /// One of [`SOURCES`](crate::SOURCES).
    pub source: String,
}

impl RequestSubscription {
    /// The rules [`Command::request_subscription`] applies, for a form to
    /// refuse before anything else.
    pub fn validate(&self) -> Result<(), NewsletterError> {
        normalize_email(&self.email)?;
        check_source(&self.source)?;
        Ok(())
    }
}

/// What came of a request.
#[derive(Clone, PartialEq, Eq)]
pub enum RequestOutcome {
    /// Recorded: the caller mails `token`'s link to `email` (normalized) for
    /// its owner to confirm.
    ToConfirm {
        subscriber_id: String,
        email: String,
        language: Option<String>,
        token: String,
    },
    /// On the list already: nothing to mail.
    AlreadyConfirmed,
    /// A link was mailed less than [`RESEND_AFTER_SECS`] ago (or is being
    /// mailed by another node right now): nothing more to mail.
    RecentlyRequested,
}

impl std::fmt::Debug for RequestOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ToConfirm { subscriber_id, .. } => f
                .debug_struct("ToConfirm")
                .field("subscriber_id", subscriber_id)
                .field("token", &"******")
                .finish_non_exhaustive(),
            Self::AlreadyConfirmed => f.write_str("AlreadyConfirmed"),
            Self::RecentlyRequested => f.write_str("RecentlyRequested"),
        }
    }
}

/// What a followed confirmation link did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmOutcome {
    Confirmed,
    /// Confirmed before (the link followed twice): nothing changed.
    AlreadyConfirmed,
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Records that `cmd.email` asks for the shop's news, to be confirmed
    /// through the link the caller mails. The first request draws a fresh key,
    /// which `keys` holds before the event names it; later ones reseal the
    /// address under the same key, so the links of earlier mails keep
    /// working. `now` is unix seconds.
    pub async fn request_subscription(
        &self,
        keys: &dyn Keys,
        actor: &str,
        cmd: RequestSubscription,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<RequestOutcome, NewsletterError> {
        let email = normalize_email(&cmd.email)?;
        let source = check_source(&cmd.source)?;
        let language = cmd
            .language
            .as_deref()
            .and_then(timada_core::primary_language);
        let id = subscriber_id(&email);
        let current = self.load_subscriber_any(&id).await?;
        if let Some(state) = current.as_ref().filter(|state| !state.forgotten) {
            match state.status {
                SubscriberStatus::Confirmed => return Ok(RequestOutcome::AlreadyConfirmed),
                SubscriberStatus::Pending
                    if now.saturating_sub(state.requested_at) < RESEND_AFTER_SECS =>
                {
                    return Ok(RequestOutcome::RecentlyRequested);
                }
                _ => {}
            }
        }
        let (writer, key_id, key, fresh) = self.writer(keys, &id, current).await?;
        let event = Requested {
            email: keys::seal(&key, &email_aad(&id, &key_id), email.as_bytes())?,
            key_id: key_id.clone(),
            language: language.clone(),
            source: source.to_owned(),
            requested_by: actor.to_owned(),
        };
        let mut writer = writer;
        let committed = writer
            .routing_key_opt(routing_key)
            .event(&event)
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(err) = committed {
            if fresh {
                // Nothing names the key: drop it rather than keep it unused.
                let _ = keys.delete(&key_id).await;
            }
            return match err {
                // Asked at the same moment elsewhere (another node following
                // the same checkout, say): that one mails the link.
                WriteError::InvalidOriginalVersion => Ok(RequestOutcome::RecentlyRequested),
                err => Err(err.into()),
            };
        }
        let state = self
            .load_subscriber(&id)
            .await?
            .ok_or(NewsletterError::NotFound)?;
        Ok(RequestOutcome::ToConfirm {
            token: confirm_token(&key, &id, &key_id, state.requested_at),
            subscriber_id: id,
            email,
            language,
        })
    }

    /// Puts a shopper whose address is proved already (signed in with a
    /// mailed code) on the list at once: no link to follow. On the list
    /// already is a no-op.
    pub async fn subscribe_verified(
        &self,
        keys: &dyn Keys,
        actor: &str,
        email: &str,
        language: Option<String>,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let email = normalize_email(email)?;
        let language = language.as_deref().and_then(timada_core::primary_language);
        let id = subscriber_id(&email);
        let current = self.load_subscriber_any(&id).await?;
        if current.as_ref().is_some_and(SubscriberState::is_confirmed) {
            return Ok(());
        }
        let (writer, key_id, key, fresh) = self.writer(keys, &id, current).await?;
        let mut writer = writer;
        let committed = writer
            .routing_key_opt(routing_key)
            .event(&Requested {
                email: keys::seal(&key, &email_aad(&id, &key_id), email.as_bytes())?,
                key_id: key_id.clone(),
                language,
                source: SOURCE_ACCOUNT.to_owned(),
                requested_by: actor.to_owned(),
            })
            .event(&Confirmed {
                confirmed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await;
        if let Err(err) = committed {
            if fresh {
                let _ = keys.delete(&key_id).await;
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Confirms the request a mailed link names. A link that does not check
    /// out, or names a request that is no longer the pending one, is
    /// [`NewsletterError::InvalidLink`]; one older than [`CONFIRM_TTL_SECS`]
    /// is [`NewsletterError::LinkExpired`]. `now` is unix seconds.
    pub async fn confirm_subscription(
        &self,
        keys: &dyn Keys,
        actor: &str,
        token: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<ConfirmOutcome, NewsletterError> {
        let link = ConfirmLink::parse(token).ok_or(NewsletterError::InvalidLink)?;
        loop {
            let state = self
                .load_subscriber(link.subscriber_id)
                .await?
                .ok_or(NewsletterError::InvalidLink)?;
            let key = keys
                .get(&state.key_id)
                .await?
                .ok_or(NewsletterError::InvalidLink)?;
            if !link.verify(&key, &state.key_id) {
                return Err(NewsletterError::InvalidLink);
            }
            if state.status == SubscriberStatus::Confirmed {
                return Ok(ConfirmOutcome::AlreadyConfirmed);
            }
            if state.status != SubscriberStatus::Pending || state.requested_at != link.requested_at
            {
                return Err(NewsletterError::InvalidLink);
            }
            if now.saturating_sub(link.requested_at) > CONFIRM_TTL_SECS {
                return Err(NewsletterError::LinkExpired);
            }
            match state
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Confirmed {
                    confirmed_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
            {
                Ok(_) => return Ok(ConfirmOutcome::Confirmed),
                // Followed twice at once: decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Takes the subscriber a mail's link names off the list. Off it already
    /// is a no-op; a link that does not check out (or of an erased
    /// subscriber) is [`NewsletterError::InvalidLink`]. Returns the
    /// subscriber's id.
    pub async fn unsubscribe_with_link(
        &self,
        keys: &dyn Keys,
        actor: &str,
        token: &str,
        routing_key: Option<String>,
    ) -> Result<String, NewsletterError> {
        let link = UnsubscribeLink::parse(token).ok_or(NewsletterError::InvalidLink)?;
        let state = self
            .load_subscriber(link.subscriber_id)
            .await?
            .ok_or(NewsletterError::InvalidLink)?;
        let key = keys
            .get(&state.key_id)
            .await?
            .ok_or(NewsletterError::InvalidLink)?;
        if !link.verify(&key, &state.key_id) {
            return Err(NewsletterError::InvalidLink);
        }
        self.take_off(actor, &state.id, routing_key).await?;
        Ok(state.id)
    }

    /// Takes a subscriber off the list (or withdraws a pending request). Off
    /// it already is a no-op; unknown is [`NewsletterError::NotFound`].
    pub async fn unsubscribe(
        &self,
        actor: &str,
        subscriber_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        self.take_off(actor, subscriber_id, routing_key).await
    }
}

impl<E: Executor> super::Command<E> {
    async fn take_off(
        &self,
        actor: &str,
        subscriber_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        loop {
            let state = self
                .load_subscriber(subscriber_id)
                .await?
                .ok_or(NewsletterError::NotFound)?;
            if state.status == SubscriberStatus::Unsubscribed {
                return Ok(());
            }
            match state
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&Unsubscribed {
                    unsubscribed_by: actor.to_owned(),
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await
            {
                Ok(_) => return Ok(()),
                // Changed meanwhile: decide again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Where a `Requested` for `id` is written, and under which key: the
    /// subscriber's own while they are not erased, else a fresh one, held in
    /// `keys` first (`true` when fresh).
    async fn writer(
        &self,
        keys: &dyn Keys,
        id: &str,
        current: Option<SubscriberState>,
    ) -> Result<(WriteBuilder, String, Key, bool), NewsletterError> {
        match current {
            Some(state) if !state.forgotten => {
                let key = keys
                    .get(&state.key_id)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("the key of subscriber {id} is not readable"))?;
                let key_id = state.key_id.clone();
                Ok((state.write()?, key_id, key, false))
            }
            current => {
                let key_id = new_key_id();
                let key = Key::generate();
                keys.put(&key_id, &key).await?;
                let writer = match current {
                    // The address was erased: it subscribes again, afresh.
                    Some(forgotten) => forgotten.write()?,
                    None => evento::append(id).original_version(0).to_owned(),
                };
                Ok((writer, key_id, key, true))
            }
        }
    }
}
