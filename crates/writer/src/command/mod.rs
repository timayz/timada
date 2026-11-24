//! Write side: the command gateways and the strict write models they load.
//!
//! The write models fold *all* events of their aggregate (`strict()`), so
//! the version they observe, and `write()` sends as `original_version`,
//! never goes stale. None is snapshotted: a draft is three events.
//!
//! A draft's stream is written by the host on several nodes (each sees the
//! request, one takes it on), so a write that lost to another is no
//! verdict: every command loads the draft again and decides again
//! ([`WRITE_ATTEMPTS`] times).

mod spend;

use std::ops::Deref;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

pub use spend::*;

use crate::aggregator::{Brief, BriefSet, Draft, Drafted, Failed, Requested, Started};
use crate::error::WriterError;
use crate::value_object::{
    DRAFT_STALE_SECS, DraftStatus, KINDS, MAX_BRIEF_LEN, MAX_KEYWORD_LEN, MAX_REASON_LEN,
    MAX_SUBJECT_ID_LEN, MAX_SUBJECT_NAME_LEN, MAX_TO_CHECK, MAX_TO_CHECK_LEN, brief_id, cut,
    new_draft_id,
};

/// The command gateway of one shop. Cheap to clone when `E` is.
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

/// How many times a command decides again when another write to the draft
/// came in between its load and its write.
const WRITE_ATTEMPTS: usize = 5;

/// How a shop wants to be written for; empty until it says.
#[evento::projection(id = id, name = "timada-writer/BriefState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct BriefState {
    pub id: String,
    pub tone: String,
    pub never: String,
    /// Unix seconds of the last change; 0 before the first.
    pub updated_at: u64,
}

pub fn brief_projection<E: Executor>() -> Projection<E, BriefState> {
    Projection::new::<Brief>().handler(on_brief_set()).strict()
}

#[evento::handler]
async fn on_brief_set(event: Event<BriefSet>, brief: &mut BriefState) -> anyhow::Result<()> {
    brief.id = event.aggregate_id.to_owned();
    brief.tone = event.data.tone.clone();
    brief.never = event.data.never.clone();
    brief.updated_at = event.timestamp;
    Ok(())
}

/// The write model of a draft.
#[evento::projection(id = id, name = "timada-writer/DraftState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DraftState {
    pub id: String,
    pub kind: String,
    pub subject_id: String,
    pub subject_name: String,
    pub keyword: String,
    pub requested_by: String,
    pub status: DraftStatus,
    /// The proposal its texts wait as, once drafted.
    pub proposal_id: String,
    pub cost_usd_micros: u64,
    /// What the draft states that the shop's data does not.
    pub to_check: Vec<String>,
    /// Why nothing was drafted.
    pub reason: String,
    /// Unix seconds of `Requested`.
    pub requested_at: u64,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl DraftState {
    /// Whether a node took it on and never said how it went: it stopped.
    pub fn is_lost(&self, now: u64) -> bool {
        self.status == DraftStatus::Started
            && now.saturating_sub(self.updated_at) >= DRAFT_STALE_SECS
    }
}

pub fn draft_projection<E: Executor>() -> Projection<E, DraftState> {
    Projection::new::<Draft>()
        .handler(on_requested())
        .handler(on_started())
        .handler(on_drafted())
        .handler(on_failed())
        .strict()
}

#[evento::handler]
async fn on_requested(event: Event<Requested>, draft: &mut DraftState) -> anyhow::Result<()> {
    draft.id = event.aggregate_id.to_owned();
    draft.kind = event.data.kind.clone();
    draft.subject_id = event.data.subject_id.clone();
    draft.subject_name = event.data.subject_name.clone();
    draft.keyword = event.data.keyword.clone();
    draft.requested_by = event.data.requested_by.clone();
    draft.status = DraftStatus::Requested;
    draft.requested_at = event.timestamp;
    draft.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_started(event: Event<Started>, draft: &mut DraftState) -> anyhow::Result<()> {
    draft.status = DraftStatus::Started;
    draft.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_drafted(event: Event<Drafted>, draft: &mut DraftState) -> anyhow::Result<()> {
    draft.status = DraftStatus::Drafted;
    draft.proposal_id = event.data.proposal_id.clone();
    draft.cost_usd_micros = event.data.cost_usd_micros;
    draft.to_check = event.data.to_check.clone();
    draft.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_failed(event: Event<Failed>, draft: &mut DraftState) -> anyhow::Result<()> {
    draft.status = DraftStatus::Failed;
    draft.reason = event.data.reason.clone();
    draft.updated_at = event.timestamp;
    Ok(())
}

/// A draft to ask for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestDraft {
    /// One of [`KINDS`].
    pub kind: String,
    pub subject_id: String,
    /// The subject's name now, for the list.
    pub subject_name: String,
    /// The search to write for; empty to write for the subject's name.
    pub keyword: String,
}

impl<E: Executor> Command<E> {
    /// How the shop wants to be written for; empty before it said.
    pub async fn load_brief(&self) -> anyhow::Result<BriefState> {
        Ok(self
            .load_brief_state()
            .await?
            .unwrap_or_else(|| BriefState {
                id: brief_id(),
                ..BriefState::default()
            }))
    }

    async fn load_brief_state(&self) -> anyhow::Result<Option<BriefState>> {
        brief_projection()
            .load(brief_id())
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    pub async fn load_draft(&self, draft_id: &str) -> anyhow::Result<Option<DraftState>> {
        draft_projection()
            .load(draft_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    /// Loads the draft, lets `decide` say what to write, and writes it;
    /// again when another write came in between. `false` when there is no
    /// such draft, or `decide` found nothing to write.
    async fn settle<F>(
        &self,
        draft_id: &str,
        routing_key: Option<String>,
        decide: F,
    ) -> Result<bool, WriterError>
    where
        F: Fn(&DraftState, &mut evento::WriteBuilder) -> bool,
    {
        for _ in 0..WRITE_ATTEMPTS {
            let Some(draft) = self.load_draft(draft_id).await? else {
                return Ok(false);
            };
            let mut write = draft.write()?;
            if !decide(&draft, &mut write) {
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
    /// Says how the shop wants to be written for; the same again is a
    /// no-op.
    pub async fn set_brief(
        &self,
        actor: &str,
        tone: &str,
        never: &str,
        routing_key: Option<String>,
    ) -> Result<(), WriterError> {
        let (tone, never) = (tone.trim(), never.trim());
        if tone.chars().count() > MAX_BRIEF_LEN || never.chars().count() > MAX_BRIEF_LEN {
            return Err(WriterError::BriefTooLong);
        }
        for _ in 0..WRITE_ATTEMPTS {
            let loaded = self.load_brief_state().await?;
            if loaded
                .as_ref()
                .is_some_and(|brief| brief.tone == tone && brief.never == never)
                || (loaded.is_none() && tone.is_empty() && never.is_empty())
            {
                return Ok(());
            }
            let mut write = match &loaded {
                Some(brief) => brief.write()?,
                None => {
                    let mut write = evento::append(brief_id());
                    write.original_version(0);
                    write
                }
            };
            write.routing_key_opt(routing_key.clone()).event(&BriefSet {
                tone: tone.to_owned(),
                never: never.to_owned(),
                set_by: actor.to_owned(),
            });
            match write.tenant(&self.tenant).commit(&self.executor).await {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }

    /// Asks for the texts of a product or a category to be drafted, under
    /// a fresh id, which it returns. The host checked beforehand that the
    /// subject exists.
    pub async fn request_draft(
        &self,
        actor: &str,
        cmd: RequestDraft,
        routing_key: Option<String>,
    ) -> Result<String, WriterError> {
        if !KINDS.contains(&cmd.kind.as_str()) {
            return Err(WriterError::UnknownKind);
        }
        let subject_id = cmd.subject_id.trim();
        if subject_id.is_empty()
            || subject_id.len() > MAX_SUBJECT_ID_LEN
            || subject_id.chars().any(char::is_whitespace)
        {
            return Err(WriterError::InvalidSubject);
        }
        let keyword = cmd.keyword.trim();
        if keyword.chars().count() > MAX_KEYWORD_LEN {
            return Err(WriterError::InvalidKeyword);
        }
        let id = new_draft_id();
        let mut write = evento::append(&id);
        write
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Requested {
                kind: cmd.kind,
                subject_id: subject_id.to_owned(),
                subject_name: cut(&cmd.subject_name, MAX_SUBJECT_NAME_LEN),
                keyword: keyword.to_owned(),
                requested_by: actor.to_owned(),
            });
        write.tenant(&self.tenant).commit(&self.executor).await?;
        Ok(id)
    }

    /// Takes a requested draft on; `false` when another node did first, or
    /// there is nothing to draft any more.
    pub async fn start_draft(
        &self,
        actor: &str,
        draft_id: &str,
        routing_key: Option<String>,
    ) -> Result<bool, WriterError> {
        self.settle(draft_id, routing_key, |draft, write| {
            if draft.status != DraftStatus::Requested {
                return false;
            }
            write.event(&Started {
                started_by: actor.to_owned(),
            });
            true
        })
        .await
    }

    /// Records that the texts wait as the proposal `proposal_id`, what the
    /// call cost and what the draft states that the shop's data does not.
    /// Whether it was written.
    pub async fn record_draft(
        &self,
        actor: &str,
        draft_id: &str,
        proposal_id: &str,
        cost_usd_micros: u64,
        to_check: Vec<String>,
        routing_key: Option<String>,
    ) -> Result<bool, WriterError> {
        let to_check: Vec<String> = to_check
            .iter()
            .take(MAX_TO_CHECK)
            .map(|what| cut(what, MAX_TO_CHECK_LEN))
            .collect();
        self.settle(draft_id, routing_key, |draft, write| {
            if draft.status != DraftStatus::Started {
                return false;
            }
            write.event(&Drafted {
                proposal_id: proposal_id.to_owned(),
                cost_usd_micros,
                to_check: to_check.clone(),
                drafted_by: actor.to_owned(),
            });
            true
        })
        .await
    }

    /// Records that nothing was drafted, with why. Whether it was written:
    /// a draft that ended already is left alone.
    pub async fn record_draft_failure(
        &self,
        actor: &str,
        draft_id: &str,
        reason: &str,
        routing_key: Option<String>,
    ) -> Result<bool, WriterError> {
        let reason = cut(reason, MAX_REASON_LEN);
        self.settle(draft_id, routing_key, |draft, write| {
            if draft.status.is_settled() {
                return false;
            }
            write.event(&Failed {
                reason: reason.clone(),
                failed_by: actor.to_owned(),
            });
            true
        })
        .await
    }
}
