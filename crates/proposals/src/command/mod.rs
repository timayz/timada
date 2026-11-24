//! Write side: the command gateway and the strict write models it loads.
//!
//! The write models fold *all* events of their aggregate (`strict()`), so
//! the version they observe, and `write()` sends as `original_version`,
//! never goes stale. Neither is snapshotted: a proposal is a handful of
//! events.
//!
//! A proposal's stream is written by a member and by the host at the same
//! time, and by the host on several nodes (each applies what was approved
//! and says so), so a write that lost to another is no verdict: every
//! command loads the proposal again and decides again ([`WRITE_ATTEMPTS`]
//! times).

mod notice;

use std::ops::Deref;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};
use timada_core::Sending;

use crate::aggregator::{
    Applied, ApplyFailed, Approved, Change, FoundOutOfDate, MailClaimed, MailDeferred, MailFailed,
    MailSent, MailUndeliverable, Notice, Owed, Prepared, Proposal, Refused,
};
use crate::error::ProposalError;
use crate::value_object::{
    DEFAULT_VALID_SECS, KINDS, MAX_CHANGES, MAX_REASON_LEN, MAX_SOURCE_LEN, MAX_SUBJECT_ID_LEN,
    MAX_SUBJECT_NAME_LEN, MAX_VALID_SECS, MAX_VALUE_LEN, ProposalStatus, checked, is_field_of,
    new_proposal_id, note,
};

/// The command gateway. Cheap to clone when `E` is.
pub struct Command<E: Executor> {
    executor: E,
    tenant: String,
}

impl<E: Executor> Command<E> {
    /// The gateway of one tenant: every stream it writes or loads is that
    /// tenant's. `tenant` must not be empty (the default tenant holds the
    /// platform's own streams).
    pub fn new(executor: E, tenant: impl Into<String>) -> Self {
        let tenant = tenant.into();
        debug_assert!(!tenant.is_empty(), "a tenant gateway needs a tenant");
        Self { executor, tenant }
    }

    /// The tenant every call acts in.
    pub fn tenant(&self) -> &str {
        &self.tenant
    }
}

impl<E: Executor> Deref for Command<E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.executor
    }
}

/// How many times a command decides again when another write to the
/// proposal came in between its load and its write.
const WRITE_ATTEMPTS: usize = 5;

/// The write model of a proposal.
#[evento::projection(id = id, name = "timada-proposals/ProposalState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ProposalState {
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    pub subject_name: String,
    /// What it would change: as prepared while it waits, as approved once a
    /// member said yes.
    pub changes: Vec<Change>,
    /// Whether the member who approved it changed a value first.
    pub edited: bool,
    pub reason: String,
    pub source: String,
    pub prepared_by: String,
    /// Unix seconds after which it is no longer worth approving.
    pub valid_until: u64,
    pub status: ProposalStatus,
    /// Who approved or refused it; empty while it waits, and for one found
    /// out of date before anyone answered.
    pub answered_by: String,
    /// What ended it, in words: why it was refused, what the field held
    /// instead, why the shop refused the change.
    pub note: String,
    /// Unix seconds of `Prepared`.
    pub prepared_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl ProposalState {
    /// Whether it waited past its date at `now` (unix seconds): it can
    /// still be refused, no longer approved.
    pub fn is_expired(&self, now: u64) -> bool {
        self.status == ProposalStatus::Waiting && now > self.valid_until
    }

    /// The change it holds of `field`.
    pub fn change(&self, field: &str) -> Option<&Change> {
        self.changes.iter().find(|change| change.field == field)
    }
}

pub fn proposal_projection<E: Executor>() -> Projection<E, ProposalState> {
    Projection::new::<Proposal>()
        .handler(on_prepared())
        .handler(on_approved())
        .handler(on_refused())
        .handler(on_applied())
        .handler(on_found_out_of_date())
        .handler(on_apply_failed())
        .strict()
}

#[evento::handler]
async fn on_prepared(event: Event<Prepared>, proposal: &mut ProposalState) -> anyhow::Result<()> {
    proposal.id = event.aggregate_id.to_owned();
    proposal.kind = event.data.kind.clone();
    proposal.subject_id = event.data.subject_id.clone();
    proposal.subject_name = event.data.subject_name.clone();
    proposal.changes = event.data.changes.clone();
    proposal.reason = event.data.reason.clone();
    proposal.source = event.data.source.clone();
    proposal.prepared_by = event.data.prepared_by.clone();
    proposal.valid_until = event.data.valid_until;
    proposal.status = ProposalStatus::Waiting;
    proposal.prepared_at = event.timestamp;
    proposal.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_approved(event: Event<Approved>, proposal: &mut ProposalState) -> anyhow::Result<()> {
    proposal.edited = proposal.changes != event.data.changes;
    proposal.changes = event.data.changes.clone();
    proposal.status = ProposalStatus::Approved;
    proposal.answered_by = event.data.approved_by.clone();
    proposal.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_refused(event: Event<Refused>, proposal: &mut ProposalState) -> anyhow::Result<()> {
    proposal.status = ProposalStatus::Refused;
    proposal.answered_by = event.data.refused_by.clone();
    proposal.note = event.data.reason.clone();
    proposal.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_applied(event: Event<Applied>, proposal: &mut ProposalState) -> anyhow::Result<()> {
    proposal.status = ProposalStatus::Applied;
    proposal.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_found_out_of_date(
    event: Event<FoundOutOfDate>,
    proposal: &mut ProposalState,
) -> anyhow::Result<()> {
    proposal.status = ProposalStatus::OutOfDate;
    proposal.note = format!("{}: {}", event.data.field, event.data.found);
    proposal.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_apply_failed(
    event: Event<ApplyFailed>,
    proposal: &mut ProposalState,
) -> anyhow::Result<()> {
    proposal.status = ProposalStatus::Failed;
    proposal.note = event.data.reason.clone();
    proposal.updated_at = event.timestamp;
    Ok(())
}

/// Where the notice of a day's waiting proposals stands. No stream is no
/// claim yet.
#[evento::projection(id = id, name = "timada-proposals/NoticeState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct NoticeState {
    pub id: String,
    /// `2026-11-24`.
    pub day: String,
    /// Unix seconds: no retry is made past it.
    pub expires_at: u64,
    pub sending: Sending,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

pub fn notice_projection<E: Executor>() -> Projection<E, NoticeState> {
    Projection::new::<Notice>()
        .handler(on_notice_owed())
        .handler(on_notice_claimed())
        .handler(on_notice_sent())
        .handler(on_notice_undeliverable())
        .handler(on_notice_deferred())
        .handler(on_notice_failed())
        .strict()
}

#[evento::handler]
async fn on_notice_owed(event: Event<Owed>, state: &mut NoticeState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.day = event.data.day.clone();
    state.expires_at = event.data.expires_at;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_claimed(
    event: Event<MailClaimed>,
    state: &mut NoticeState,
) -> anyhow::Result<()> {
    state.sending = state.sending.claimed(&event.data.by, event.data.until);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_sent(event: Event<MailSent>, state: &mut NoticeState) -> anyhow::Result<()> {
    state.sending = Sending::Sent;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_undeliverable(
    event: Event<MailUndeliverable>,
    state: &mut NoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Undeliverable {
        reason: event.data.reason.clone(),
    };
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_deferred(
    event: Event<MailDeferred>,
    state: &mut NoticeState,
) -> anyhow::Result<()> {
    state.sending = Sending::Deferred {
        retry_at: event.data.retry_at,
        attempts: event.data.attempts,
        reason: event.data.reason.clone(),
    };
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_notice_failed(event: Event<MailFailed>, state: &mut NoticeState) -> anyhow::Result<()> {
    state.sending = Sending::Failed {
        reason: event.data.reason.clone(),
        attempts: event.data.attempts,
    };
    state.updated_at = event.timestamp;
    Ok(())
}

/// A change to prepare.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prepare {
    /// One of [`KINDS`].
    pub kind: String,
    pub subject_id: String,
    /// The subject's name now, for the list.
    pub subject_name: String,
    /// The fields to change, each with what it holds now and what it would
    /// take. One that would take what it holds already is left out.
    pub changes: Vec<Change>,
    pub reason: String,
    /// What prepares it.
    pub source: String,
    /// Unix seconds after which it is no longer worth approving; `None`
    /// for [`DEFAULT_VALID_SECS`] from now.
    pub valid_until: Option<u64>,
}

/// The changes of a proposal as an event keeps them: each field of `kind`
/// once, the ones that change nothing left out.
fn checked_changes(kind: &str, changes: Vec<Change>) -> Result<Vec<Change>, ProposalError> {
    if changes.is_empty() || changes.len() > MAX_CHANGES {
        return Err(ProposalError::InvalidChanges);
    }
    let mut kept: Vec<Change> = Vec::with_capacity(changes.len());
    for change in changes {
        if !is_field_of(kind, &change.field) || kept.iter().any(|k| k.field == change.field) {
            return Err(ProposalError::InvalidChanges);
        }
        if change.before.chars().count() > MAX_VALUE_LEN
            || change.after.chars().count() > MAX_VALUE_LEN
        {
            return Err(ProposalError::ValueTooLong);
        }
        if change.before != change.after {
            kept.push(change);
        }
    }
    if kept.is_empty() {
        return Err(ProposalError::NothingToChange);
    }
    Ok(kept)
}

impl<E: Executor> Command<E> {
    pub async fn load_proposal(&self, proposal_id: &str) -> anyhow::Result<Option<ProposalState>> {
        proposal_projection()
            .load(proposal_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// Loads the proposal, lets `decide` say what to write, and writes it;
    /// again when another write came in between. `decide` returns `false`
    /// when there is nothing to write.
    async fn settle<F>(
        &self,
        proposal_id: &str,
        routing_key: Option<String>,
        decide: F,
    ) -> Result<bool, ProposalError>
    where
        F: Fn(&ProposalState, &mut evento::WriteBuilder) -> Result<bool, ProposalError>,
    {
        for _ in 0..WRITE_ATTEMPTS {
            let proposal = self
                .load_proposal(proposal_id)
                .await?
                .ok_or(ProposalError::NotFound)?;
            let mut write = proposal.write()?;
            if !decide(&proposal, &mut write)? {
                return Ok(false);
            }
            write.routing_key_opt(routing_key.clone());
            match write.tenant(&self.tenant).commit(&self.executor).await {
                Ok(_) => return Ok(true),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Records a change that waits for a person, at `now` (unix seconds),
    /// under a fresh id, which it returns. The host checked beforehand
    /// that the subject exists and that each `before` is what it holds.
    pub async fn prepare(
        &self,
        actor: &str,
        cmd: Prepare,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<String, ProposalError> {
        if !KINDS.contains(&cmd.kind.as_str()) {
            return Err(ProposalError::UnknownKind);
        }
        let subject_id = checked(
            &cmd.subject_id,
            MAX_SUBJECT_ID_LEN,
            ProposalError::InvalidSubject,
        )?;
        if subject_id.chars().any(char::is_whitespace) {
            return Err(ProposalError::InvalidSubject);
        }
        let source = checked(&cmd.source, MAX_SOURCE_LEN, ProposalError::InvalidSource)?;
        if source.is_empty() {
            return Err(ProposalError::InvalidSource);
        }
        let reason = checked(&cmd.reason, MAX_REASON_LEN, ProposalError::ReasonTooLong)?;
        let changes = checked_changes(&cmd.kind, cmd.changes)?;
        let valid_until = cmd.valid_until.unwrap_or(now + DEFAULT_VALID_SECS);
        if valid_until <= now || valid_until > now + MAX_VALID_SECS {
            return Err(ProposalError::InvalidValidity);
        }
        let id = new_proposal_id();
        let mut write = evento::append(&id);
        write
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Prepared {
                kind: cmd.kind,
                subject_id,
                subject_name: cmd
                    .subject_name
                    .trim()
                    .chars()
                    .take(MAX_SUBJECT_NAME_LEN)
                    .collect(),
                changes,
                reason,
                source,
                valid_until,
                prepared_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Says yes to a waiting proposal at `now` (unix seconds). `edits`
    /// gives, by field, the value the member changed it to; a field left
    /// out keeps the value prepared, and one set back to what the subject
    /// holds is dropped from the proposal. The host checked the values
    /// beforehand, and applies them once this is recorded.
    pub async fn approve(
        &self,
        actor: &str,
        proposal_id: &str,
        edits: Vec<(String, String)>,
        now: u64,
        routing_key: Option<String>,
    ) -> Result<(), ProposalError> {
        self.settle(proposal_id, routing_key, |proposal, write| {
            if proposal.status != ProposalStatus::Waiting {
                return Err(ProposalError::NotWaiting);
            }
            if proposal.is_expired(now) {
                return Err(ProposalError::Expired);
            }
            let mut changes = proposal.changes.clone();
            for (field, after) in &edits {
                let change = changes
                    .iter_mut()
                    .find(|change| &change.field == field)
                    .ok_or(ProposalError::UnknownField)?;
                change.after = after.clone();
            }
            let changes = checked_changes(&proposal.kind, changes)?;
            write.event(&Approved {
                changes,
                approved_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Says no to a waiting proposal, one past its date included.
    pub async fn refuse(
        &self,
        actor: &str,
        proposal_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<(), ProposalError> {
        let reason = note(reason);
        self.settle(proposal_id, routing_key, |proposal, write| {
            if proposal.status != ProposalStatus::Waiting {
                return Err(ProposalError::NotWaiting);
            }
            write.event(&Refused {
                reason: reason.clone(),
                refused_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await?;
        Ok(())
    }

    /// Records that the approved changes were made. Whether it was
    /// written: a proposal another node settled already is left alone.
    pub async fn record_applied(
        &self,
        actor: &str,
        proposal_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ProposalError> {
        self.settle(proposal_id, routing_key, |proposal, write| {
            if proposal.status != ProposalStatus::Approved {
                return Ok(false);
            }
            write.event(&Applied {
                applied_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }

    /// Records that `field` holds `found`, which is neither what the
    /// proposal was prepared against nor what it would write: the proposal
    /// ends without being applied. From a proposal that waits (a member
    /// about to approve it is told) or one approved (the host found out
    /// while applying). Whether it was written.
    pub async fn record_out_of_date(
        &self,
        actor: &str,
        proposal_id: &str,
        field: &str,
        found: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ProposalError> {
        let found = note(found);
        self.settle(proposal_id, routing_key, |proposal, write| {
            if !matches!(
                proposal.status,
                ProposalStatus::Waiting | ProposalStatus::Approved
            ) {
                return Ok(false);
            }
            write.event(&FoundOutOfDate {
                field: field.to_owned(),
                found: found.clone(),
                noticed_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }

    /// Records that the shop refused the approved change, with why.
    /// Whether it was written.
    pub async fn record_apply_failure(
        &self,
        actor: &str,
        proposal_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, ProposalError> {
        let reason = note(reason);
        self.settle(proposal_id, routing_key, |proposal, write| {
            if proposal.status != ProposalStatus::Approved {
                return Ok(false);
            }
            write.event(&ApplyFailed {
                reason: reason.clone(),
                failed_by: actor.to_owned(),
            });
            Ok(true)
        })
        .await
    }
}
