//! What a user says of the sites that hold the first results: a specialist
//! shop, or a generalist. The marks are the user's, not a study's: a site
//! marked while looking at one niche is known on the next, so a marketplace
//! is marked once.

use std::collections::BTreeMap;

use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::{DomainMarked, DomainMarks};
use crate::error::ResearchError;
use crate::value_object::{MAX_MARK_EVENTS, MAX_MARKS, normalize_domain};

use super::Command;

/// How many times a mark decides again when another of the user's came in
/// between: two tabs at the most.
const MARK_WRITE_ATTEMPTS: usize = 5;

/// The write model of a user's marks.
#[evento::projection(id = id, name = "timada-research/DomainMarksState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct DomainMarksState {
    /// The user.
    pub id: String,
    /// By site: `true` for a specialist shop, `false` for a generalist.
    pub marks: BTreeMap<String, bool>,
}

impl DomainMarksState {
    /// What the user said of `domain`; `None` until they did.
    pub fn specialist(&self, domain: &str) -> Option<bool> {
        self.marks.get(domain).copied()
    }
}

pub fn domain_marks_projection<E: Executor>() -> Projection<E, DomainMarksState> {
    Projection::new::<DomainMarks>()
        .handler(on_domain_marked())
        .strict()
}

#[evento::handler]
async fn on_domain_marked(
    event: Event<DomainMarked>,
    marks: &mut DomainMarksState,
) -> anyhow::Result<()> {
    marks.id = event.aggregate_id.to_owned();
    marks
        .marks
        .insert(event.data.domain.to_owned(), event.data.specialist);
    Ok(())
}

impl<E: Executor> Command<E> {
    /// A user's marks as they stand; none before their first.
    pub async fn load_marks(&self, user_id: &str) -> anyhow::Result<DomainMarksState> {
        Ok(domain_marks_projection()
            .load(user_id)
            .execute(&self.0)
            .await?
            .unwrap_or_else(|| DomainMarksState {
                id: user_id.to_owned(),
                ..DomainMarksState::default()
            }))
    }
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Marks `domain` for the actor as a specialist shop or as a
    /// generalist, on every study of theirs; the same as it stands is a
    /// no-op.
    pub async fn mark_domain(
        &self,
        actor_user_id: &str,
        domain: &str,
        specialist: bool,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let domain = normalize_domain(domain)?;
        for _ in 0..MARK_WRITE_ATTEMPTS {
            let loaded = domain_marks_projection()
                .load(actor_user_id)
                .execute(&self.0)
                .await?;
            let mut write = match &loaded {
                Some(marks) => {
                    if marks.specialist(&domain) == Some(specialist) {
                        return Ok(());
                    }
                    if (marks.specialist(&domain).is_none() && marks.marks.len() >= MAX_MARKS)
                        || marks.aggregate_version >= MAX_MARK_EVENTS
                    {
                        return Err(ResearchError::TooManyMarks);
                    }
                    marks.write()?
                }
                None => {
                    let mut write = evento::append(actor_user_id);
                    write.original_version(0);
                    write
                }
            };
            write
                .routing_key_opt(routing_key.clone())
                .event(&DomainMarked {
                    domain: domain.clone(),
                    specialist,
                    marked_by: actor_user_id.to_owned(),
                });
            match write.commit(&self.0).await {
                Ok(_) => return Ok(()),
                Err(WriteError::InvalidOriginalVersion) => continue,
                Err(err) => return Err(err.into()),
            }
        }
        Err(WriteError::InvalidOriginalVersion.into())
    }
}
