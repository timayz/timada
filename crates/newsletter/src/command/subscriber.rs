use evento::{Executor, ProjectionAggregate, WriteBuilder, WriteError};
use timada_core::keys::{self, Key, Keys, new_key_id};
use timada_core::{Claim, Claimant, LEASE_SECS};

use crate::aggregator::{
    Confirmed, MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, Requested,
    TimeZoneSet, Unsubscribed,
};
use crate::error::NewsletterError;
use crate::value_object::{
    CONFIRM_TTL_SECS, ConfirmLink, RESEND_AFTER_SECS, SOURCE_ACCOUNT, SubscriberStatus,
    UnsubscribeLink, check_source, confirm_token, normalize_email, subscriber_id,
};

use super::{SubscriberState, email_aad};

/// Reload-and-retry budget for a write on a subscriber that raced another
/// (a time zone landing while the mail is claimed, say).
const MAIL_WRITE_ATTEMPTS: usize = 3;

/// How a send went, for the stream.
#[derive(Clone, Copy)]
enum MailOutcome<'a> {
    Sent,
    Undeliverable(&'a str),
    NotSent { reason: &'a str, now: u64 },
}

/// What a shopper asks for: the shop's news at `email`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestSubscription {
    pub email: String,
    /// A language tag or a whole `Accept-Language` value; only the primary
    /// subtag is kept.
    pub language: Option<String>,
    /// One of [`SOURCES`](crate::SOURCES).
    pub source: String,
    /// The IANA time zone the shopper's browser gave; anything else is
    /// ignored, as is a blank.
    pub time_zone: Option<String>,
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

/// What the node that took the sending of a confirmation puts in the mail.
#[derive(Clone, PartialEq, Eq)]
pub struct ConfirmationMail {
    /// The normalized address, opened.
    pub email: String,
    pub language: Option<String>,
    /// The token of the link that confirms the request.
    pub token: String,
}

impl std::fmt::Debug for ConfirmationMail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfirmationMail")
            .field("email", &self.email)
            .field("language", &self.language)
            .field("token", &"******")
            .finish()
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
    /// through the link the subscription that follows the stream mails
    /// ([`Self::claim_confirmation_mail`]). The first request draws a fresh
    /// key, which `keys` holds before the event names it; later ones reseal
    /// the address under the same key, so the links of earlier mails keep
    /// working. A request within [`RESEND_AFTER_SECS`] of a pending one is
    /// not recorded, unless that one's mail could not be delivered: then the
    /// shopper may try again at once. `now` is unix seconds.
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
        let time_zone = cmd.time_zone.as_deref().and_then(timada_core::time_zone);
        let id = subscriber_id(&email);
        let current = self.load_subscriber_any(&id).await?;
        if let Some(state) = current.as_ref().filter(|state| !state.forgotten) {
            match state.status {
                SubscriberStatus::Confirmed => {
                    // On the list already, maybe from somewhere else now.
                    if let Some(time_zone) = time_zone {
                        self.set_time_zone(actor, &id, &time_zone, routing_key)
                            .await?;
                    }
                    return Ok(RequestOutcome::AlreadyConfirmed);
                }
                SubscriberStatus::Pending
                    if now.saturating_sub(state.requested_at) < RESEND_AFTER_SECS
                        && !state.mail.is_undeliverable() =>
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
        writer.routing_key_opt(routing_key).event(&event);
        if let Some(time_zone) = time_zone {
            writer.event(&TimeZoneSet {
                time_zone,
                set_by: actor.to_owned(),
            });
        }
        let committed = writer.tenant(&self.tenant).commit(&self.executor).await;
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
        time_zone: Option<String>,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let email = normalize_email(email)?;
        let language = language.as_deref().and_then(timada_core::primary_language);
        let time_zone = time_zone.as_deref().and_then(timada_core::time_zone);
        let id = subscriber_id(&email);
        let current = self.load_subscriber_any(&id).await?;
        if current.as_ref().is_some_and(SubscriberState::is_confirmed) {
            if let Some(time_zone) = time_zone {
                self.set_time_zone(actor, &id, &time_zone, routing_key)
                    .await?;
            }
            return Ok(());
        }
        let (writer, key_id, key, fresh) = self.writer(keys, &id, current).await?;
        let mut writer = writer;
        writer
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
            });
        if let Some(time_zone) = time_zone {
            writer.event(&TimeZoneSet {
                time_zone,
                set_by: actor.to_owned(),
            });
        }
        let committed = writer.tenant(&self.tenant).commit(&self.executor).await;
        if let Err(err) = committed {
            if fresh {
                let _ = keys.delete(&key_id).await;
            }
            return Err(err.into());
        }
        Ok(())
    }

    /// Takes, for `claimant`, the sending of the confirmation link of
    /// the request `Requested` at `requested_version` of subscriber
    /// `subscriber_id` recorded: what to mail, with the address opened by its
    /// key from `keys`, or `None` when this node mails nothing (the outcome
    /// is recorded, another node is sending, the next try is not due, the
    /// request is no longer the pending one or its link would be expired
    /// ([`CONFIRM_TTL_SECS`]), or the subscriber is erased). A node that
    /// holds the claim gets the mail again, to send it again after a failure
    /// worth a retry.
    pub async fn claim_confirmation_mail(
        &self,
        keys: &dyn Keys,
        subscriber_id: &str,
        requested_version: u16,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<Option<ConfirmationMail>, NewsletterError> {
        let Claimant { node, now } = claimant;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(state) = self.load_subscriber(subscriber_id).await? else {
                return Ok(None);
            };
            if state.requested_version != requested_version
                || state.status != SubscriberStatus::Pending
                || now.saturating_sub(state.requested_at) > CONFIRM_TTL_SECS
            {
                return Ok(None);
            }
            match state.mail.claim(node, now) {
                Claim::Leave => return Ok(None),
                Claim::Held => return confirmation_mail(keys, &state).await,
                Claim::Take => {}
            }
            // Opened before the claim: a key gone leaves nothing to claim.
            let Some(mail) = confirmation_mail(keys, &state).await? else {
                return Ok(None);
            };
            let claimed = state
                .write()?
                .routing_key_opt(routing_key.clone())
                .event(&MailClaimed {
                    by: node.to_owned(),
                    until: now + LEASE_SECS,
                })
                .tenant(&self.tenant)
                .commit(&self.executor)
                .await;
            match claimed {
                Ok(_) => return Ok(Some(mail)),
                // Another node claimed it, or the subscriber changed: decide
                // again on the fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < MAIL_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Records that the link of the request `Requested` at
    /// `requested_version` recorded was handed to the mail server. Nothing is
    /// written once the request was replaced, or the outcome recorded
    /// already.
    pub async fn record_confirmation_mail_sent(
        &self,
        subscriber_id: &str,
        requested_version: u16,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        self.record_confirmation_mail(
            subscriber_id,
            requested_version,
            MailOutcome::Sent,
            routing_key,
        )
        .await
    }

    /// Records that the send failed at `now` (unix seconds) for `reason`, a
    /// reason worth a retry (the shop's server did not answer, a transient
    /// refusal): deferred to the next try of the schedule
    /// ([`timada_core::RETRY_DELAYS_SECS`]), or given up on once none is
    /// left. Any node may take the next try once it is due.
    pub async fn record_confirmation_mail_not_sent(
        &self,
        subscriber_id: &str,
        requested_version: u16,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        self.record_confirmation_mail(
            subscriber_id,
            requested_version,
            MailOutcome::NotSent { reason, now },
            routing_key,
        )
        .await
    }

    /// Records that the mail server refused the address for good (or that it
    /// does not parse), with what it said. The address may then ask again at
    /// once.
    pub async fn record_confirmation_mail_undeliverable(
        &self,
        subscriber_id: &str,
        requested_version: u16,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        self.record_confirmation_mail(
            subscriber_id,
            requested_version,
            MailOutcome::Undeliverable(reason),
            routing_key,
        )
        .await
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

/// The mail of a subscriber's pending request: `None` once their key is
/// destroyed; an error when the key cannot be read yet.
async fn confirmation_mail(
    keys: &dyn Keys,
    state: &SubscriberState,
) -> Result<Option<ConfirmationMail>, NewsletterError> {
    let Some(key) = keys.get(&state.key_id).await? else {
        return Ok(None);
    };
    let subscriber = state.open(&key)?;
    Ok(Some(ConfirmationMail {
        email: subscriber.email,
        language: subscriber.language,
        token: confirm_token(&key, &state.id, &state.key_id, state.requested_at),
    }))
}

impl<E: Executor> super::Command<E> {
    async fn record_confirmation_mail(
        &self,
        subscriber_id: &str,
        requested_version: u16,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            let Some(state) = self.load_subscriber(subscriber_id).await? else {
                return Ok(());
            };
            if state.requested_version != requested_version || state.mail.is_settled() {
                return Ok(());
            }
            let mut writer = state.write()?;
            writer.routing_key_opt(routing_key.clone());
            match outcome {
                MailOutcome::Sent => writer.event(&MailSent),
                MailOutcome::Undeliverable(reason) => writer.event(&MailUndeliverable {
                    reason: reason.to_owned(),
                }),
                MailOutcome::NotSent { reason, now } => match state.mail.retry_at(now) {
                    Some(retry_at) => writer.event(&MailDeferred {
                        retry_at,
                        reason: reason.to_owned(),
                    }),
                    None => writer.event(&MailFailed {
                        reason: reason.to_owned(),
                    }),
                },
            };
            match writer.tenant(&self.tenant).commit(&self.executor).await {
                Ok(_) => return Ok(()),
                // The subscriber changed meanwhile: write past it.
                Err(WriteError::InvalidOriginalVersion) if attempt < MAIL_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Records where a subscriber on the list lives now; the same zone, or
    /// a write that lost a race (the next request says it again), is a
    /// no-op.
    async fn set_time_zone(
        &self,
        actor: &str,
        subscriber_id: &str,
        time_zone: &str,
        routing_key: Option<String>,
    ) -> Result<(), NewsletterError> {
        let Some(state) = self.load_subscriber(subscriber_id).await? else {
            return Ok(());
        };
        if state.time_zone.as_deref() == Some(time_zone) {
            return Ok(());
        }
        match state
            .write()?
            .routing_key_opt(routing_key)
            .event(&TimeZoneSet {
                time_zone: time_zone.to_owned(),
                set_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
        {
            Ok(_) | Err(WriteError::InvalidOriginalVersion) => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

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
