//! The notice that a checkpoint's day came, mailed to the shop's owners by
//! a subscription on every node of the work group: the node that writes
//! the claim on the checkpoint's notice first is the one that mails, then
//! records how it went ([`timada_core::Sending`]), so the owners are told
//! once and a send the server fails is tried again later, by whichever
//! node looks once the retry is due.

use evento::{Executor, ProjectionAggregate, WriteError};
use timada_core::{Claim, Claimant, LEASE_SECS, MailOutcome, MailRecord};

use crate::aggregator::{MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, Owed};
use crate::error::LaunchError;
use crate::value_object::notice_id;

use super::{NoticeState, notice_projection};

/// Reload-and-retry budget for a write on a notice's stream that raced
/// another node's.
const MAIL_WRITE_ATTEMPTS: usize = 3;

impl<E: Executor> super::Command<E> {
    /// Where the notice of `checkpoint` stands, once a node claimed it.
    pub async fn load_notice(&self, checkpoint: u32) -> anyhow::Result<Option<NoticeState>> {
        notice_projection()
            .load(notice_id(checkpoint))
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    async fn record_notice(
        &self,
        checkpoint: u32,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            // No claim, nothing to record over.
            let Some(state) = self.load_notice(checkpoint).await? else {
                return Ok(());
            };
            let record = state.sending.record(outcome, state.expires_at);
            if record == MailRecord::Nothing {
                return Ok(());
            }
            let mut write = state.write()?;
            write.routing_key_opt(routing_key.clone());
            match record {
                MailRecord::Sent => write.event(&MailSent),
                MailRecord::Undeliverable(reason) => write.event(&MailUndeliverable { reason }),
                MailRecord::Deferred {
                    retry_at,
                    attempts,
                    reason,
                } => write.event(&MailDeferred {
                    retry_at,
                    attempts,
                    reason,
                }),
                MailRecord::Failed { reason, attempts } => {
                    write.event(&MailFailed { reason, attempts })
                }
                MailRecord::Nothing => unreachable!("returned above"),
            };
            match write.tenant(&self.tenant).commit(&self.executor).await {
                Ok(_) => return Ok(()),
                // Another node wrote meanwhile: decide again on its word.
                Err(WriteError::InvalidOriginalVersion) if attempt < MAIL_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Takes, for `claimant`, the telling of `checkpoint`,
    /// worth sending until `expires_at` (unix seconds). `true` when this
    /// node mails: it took the claim, or holds it already (a send it
    /// retries); `false` when another node is sending, the next try is not
    /// due yet, or the outcome is recorded. The first claim writes what the
    /// stream is about ([`Owed`]) with it.
    pub async fn claim_notice(
        &self,
        checkpoint: u32,
        expires_at: u64,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<bool, LaunchError> {
        let Claimant { node, now } = claimant;
        let id = notice_id(checkpoint);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let state = self.load_notice(checkpoint).await?;
            let sending = state.as_ref().map(|state| &state.sending);
            match sending.map_or(Claim::Take, |sending| sending.claim(node, now)) {
                Claim::Leave => return Ok(false),
                Claim::Held => return Ok(true),
                Claim::Take => {}
            }
            let claimed = MailClaimed {
                by: node.to_owned(),
                until: now + LEASE_SECS,
            };
            let mut write = match &state {
                Some(state) => state.write()?,
                None => {
                    let mut write = evento::append(&id);
                    write.original_version(0).event(&Owed {
                        checkpoint,
                        expires_at,
                    });
                    write
                }
            };
            write.routing_key_opt(routing_key.clone()).event(&claimed);
            match write.tenant(&self.tenant).commit(&self.executor).await {
                Ok(_) => return Ok(true),
                // Another node claimed it meanwhile: decide again on the
                // fresh state.
                Err(WriteError::InvalidOriginalVersion) if attempt < MAIL_WRITE_ATTEMPTS => {}
                Err(err) => return Err(err.into()),
            }
        }
    }

    /// Records that the notice of `checkpoint` was handed to the mail server.
    /// Nothing is written once the outcome is recorded already.
    pub async fn record_notice_sent(
        &self,
        checkpoint: u32,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        self.record_notice(checkpoint, MailOutcome::Sent, routing_key)
            .await
    }

    /// Records that the mail server refused every owner's address for
    /// good, with what it said of the first.
    pub async fn record_notice_undeliverable(
        &self,
        checkpoint: u32,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        self.record_notice(checkpoint, MailOutcome::Undeliverable(reason), routing_key)
            .await
    }

    /// Records that `node`'s send failed at `now` (unix seconds) for
    /// `reason`, a reason worth a retry: deferred to the next try of the
    /// schedule ([`timada_core::RETRY_DELAYS_SECS`]), or given up on once
    /// none is left before the notice's window closes.
    pub async fn record_notice_not_sent(
        &self,
        checkpoint: u32,
        node: &str,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), LaunchError> {
        self.record_notice(
            checkpoint,
            MailOutcome::NotSent { node, reason, now },
            routing_key,
        )
        .await
    }
}
