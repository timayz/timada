//! The mails about an order, sent by a subscription on every node of the
//! region: the node that writes the claim on the mail's stream first is the
//! one that sends, then records how it went ([`timada_core::Sending`]), so a
//! shopper gets each mail once and a send the shop's server fails is tried
//! again later, by whichever node looks once the retry is due.

use evento::{Executor, ProjectionAggregate, WriteError};
use timada_core::{Claim, Claimant, LEASE_SECS, MailOutcome, MailRecord};

use crate::aggregator::{MailClaimed, MailDeferred, MailFailed, MailSent, MailUndeliverable, Owed};
use crate::error::OrderError;
use crate::value_object::order_mail_id;

/// Reload-and-retry budget for a write on a mail's stream that raced another
/// node's.
const MAIL_WRITE_ATTEMPTS: usize = 3;

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Takes, for `claimant`, the sending of the mail `mail` (a
    /// [`ShopMailKind`](timada_tenancy::ShopMailKind) key) about `order_id`,
    /// worth sending until `expires_at` (unix seconds). `true` when this node
    /// sends: it took the claim, or holds it already (a send it retries);
    /// `false` when another node is sending, the next try is not due yet, or
    /// the outcome is recorded. The first claim writes what the stream is
    /// about ([`Owed`]) with it.
    pub async fn claim_order_mail(
        &self,
        order_id: &str,
        mail: &str,
        expires_at: u64,
        claimant: Claimant<'_>,
        routing_key: Option<String>,
    ) -> Result<bool, OrderError> {
        let Claimant { node, now } = claimant;
        let id = order_mail_id(order_id, mail);
        let mut attempt = 0;
        loop {
            attempt += 1;
            let state = self.load_order_mail(order_id, mail).await?;
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
                        order_id: order_id.to_owned(),
                        mail: mail.to_owned(),
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

    /// Records that the mail `mail` about `order_id` was handed to the mail
    /// server. Nothing is written once the outcome is recorded already.
    pub async fn record_order_mail_sent(
        &self,
        order_id: &str,
        mail: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        self.record_order_mail(order_id, mail, MailOutcome::Sent, routing_key)
            .await
    }

    /// Records that the mail server refused the shopper's address for good
    /// (or that it does not parse), with what it said: the order's page
    /// shows it.
    pub async fn record_order_mail_undeliverable(
        &self,
        order_id: &str,
        mail: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        self.record_order_mail(
            order_id,
            mail,
            MailOutcome::Undeliverable(reason),
            routing_key,
        )
        .await
    }

    /// Records that `node`'s send failed at `now` (unix seconds) for
    /// `reason`, a reason worth a retry (the server did not answer, a
    /// transient refusal): deferred to the next try of the schedule
    /// ([`timada_core::RETRY_DELAYS_SECS`]), or given up on once none is
    /// left before the mail's window closes, which the order's page shows.
    pub async fn record_order_mail_not_sent(
        &self,
        order_id: &str,
        mail: &str,
        node: &str,
        reason: &str,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        self.record_order_mail(
            order_id,
            mail,
            MailOutcome::NotSent { node, reason, now },
            routing_key,
        )
        .await
    }
}

impl<E: Executor> super::Command<E> {
    async fn record_order_mail(
        &self,
        order_id: &str,
        mail: &str,
        outcome: MailOutcome<'_>,
        routing_key: Option<String>,
    ) -> Result<(), OrderError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            // No claim, nothing to record over.
            let Some(state) = self.load_order_mail(order_id, mail).await? else {
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
