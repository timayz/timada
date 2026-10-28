use evento::{Executor, Projection, ProjectionAggregate, WriteError, metadata::Event};

use crate::aggregator::scenario::{Archived, Created, Paused, Published, Renamed, Resumed, Saved};
use crate::definition::{Definition, Problem};
use crate::error::AutomationError;
use crate::value_object::{ScenarioStatus, checked_name, new_scenario_id};

/// The write model of a scenario.
#[evento::projection(id = id, name = "timada-automations/ScenarioState")]
#[evento::snapshot(none)]
#[derive(Debug, PartialEq, Eq)]
pub struct ScenarioState {
    pub id: String,
    pub name: String,
    /// The draft as last saved or published, as JSON; empty while never
    /// saved.
    pub draft: String,
    /// The version running; 0 while never published.
    pub version: u32,
    /// The definition of that version, as JSON.
    pub live: String,
    pub status: ScenarioStatus,
    /// Unix seconds of `Created`.
    pub created_at: u64,
    /// Unix seconds of the last `Published`.
    pub published_at: Option<u64>,
    /// Unix seconds of the last event.
    pub updated_at: u64,
}

impl ScenarioState {
    /// The draft, read.
    pub fn definition(&self) -> anyhow::Result<Definition> {
        Ok(Definition::from_json(&self.draft)?)
    }

    /// The version running, read; `None` while never published.
    pub fn live_definition(&self) -> anyhow::Result<Option<Definition>> {
        if self.version == 0 {
            return Ok(None);
        }
        Ok(Some(Definition::from_json(&self.live)?))
    }

    /// Whether the draft holds what the version running does not (always,
    /// while never published).
    pub fn has_unpublished_changes(&self) -> bool {
        self.version == 0 || self.draft != self.live
    }
}

pub fn scenario_projection<E: Executor>() -> Projection<E, ScenarioState> {
    Projection::new::<crate::aggregator::scenario::Scenario>()
        .handler(on_created())
        .handler(on_renamed())
        .handler(on_saved())
        .handler(on_published())
        .handler(on_paused())
        .handler(on_resumed())
        .handler(on_archived())
        .strict()
}

#[evento::handler]
async fn on_created(event: Event<Created>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.id = event.aggregate_id.to_owned();
    state.name = event.data.name.to_owned();
    state.status = ScenarioStatus::Draft;
    state.created_at = event.timestamp;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_renamed(event: Event<Renamed>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.name = event.data.name.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_saved(event: Event<Saved>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.draft = event.data.definition.to_owned();
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_published(event: Event<Published>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.draft = event.data.definition.to_owned();
    state.live = event.data.definition.to_owned();
    state.version = event.data.version;
    state.status = ScenarioStatus::Active;
    state.published_at = Some(event.timestamp);
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_paused(event: Event<Paused>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.status = ScenarioStatus::Paused;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_resumed(event: Event<Resumed>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.status = ScenarioStatus::Active;
    state.updated_at = event.timestamp;
    Ok(())
}

#[evento::handler]
async fn on_archived(event: Event<Archived>, state: &mut ScenarioState) -> anyhow::Result<()> {
    state.status = ScenarioStatus::Archived;
    state.updated_at = event.timestamp;
    Ok(())
}

/// The problems a definition may not even be saved with.
fn hard_problems(definition: &Definition) -> Vec<Problem> {
    definition
        .check()
        .into_iter()
        .filter(Problem::is_hard)
        .collect()
}

/// A write that lost the race to another member's is a stale copy.
fn stale(err: WriteError) -> AutomationError {
    match err {
        WriteError::InvalidOriginalVersion => AutomationError::Changed,
        err => err.into(),
    }
}

#[evento::command]
impl<E: Executor> super::Command<E> {
    /// Starts a scenario with nothing in it; its id.
    pub async fn create_scenario(
        &self,
        actor: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<String, AutomationError> {
        let name = checked_name(name).ok_or(AutomationError::InvalidName)?;
        let id = new_scenario_id();
        evento::append(&id)
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                name,
                created_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await?;
        Ok(id)
    }

    /// Saves the editor's copy as the draft, whole, with the name. `base`
    /// is the scenario's version the copy was made from: another save since
    /// is [`AutomationError::Changed`]. A draft may be unfinished, only not
    /// beyond the limits. Whether anything changed.
    pub async fn save_scenario(
        &self,
        actor: &str,
        scenario_id: &str,
        base: u16,
        name: &str,
        definition: &Definition,
        routing_key: Option<String>,
    ) -> Result<bool, AutomationError> {
        let name = checked_name(name).ok_or(AutomationError::InvalidName)?;
        let hard = hard_problems(definition);
        if !hard.is_empty() {
            return Err(AutomationError::TooLarge(hard));
        }
        let state = self.require_scenario(scenario_id).await?;
        if state.status == ScenarioStatus::Archived {
            return Err(AutomationError::Archived);
        }
        if state.aggregate_version()? != base {
            return Err(AutomationError::Changed);
        }
        let json = definition.to_json();
        let renamed = name != state.name;
        let saved = json != state.draft;
        if !renamed && !saved {
            return Ok(false);
        }
        let mut write = state.write()?;
        write.routing_key_opt(routing_key);
        if renamed {
            write.event(&Renamed {
                name,
                renamed_by: actor.to_owned(),
            });
        }
        if saved {
            write.event(&Saved {
                definition: json,
                saved_by: actor.to_owned(),
            });
        }
        write
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
            .map_err(stale)?;
        Ok(true)
    }

    /// Publishes the editor's copy, with the name, as the next version; its
    /// number. Refused while [`Definition::check`] finds a problem; `base`
    /// as for [`save_scenario`](Self::save_scenario). A paused scenario runs
    /// again once published.
    pub async fn publish_scenario(
        &self,
        actor: &str,
        scenario_id: &str,
        base: u16,
        name: &str,
        definition: &Definition,
        routing_key: Option<String>,
    ) -> Result<u32, AutomationError> {
        let name = checked_name(name).ok_or(AutomationError::InvalidName)?;
        let problems = definition.check();
        if problems.iter().any(Problem::is_hard) {
            return Err(AutomationError::TooLarge(
                problems.into_iter().filter(Problem::is_hard).collect(),
            ));
        }
        if !problems.is_empty() {
            return Err(AutomationError::Invalid(problems));
        }
        let state = self.require_scenario(scenario_id).await?;
        if state.status == ScenarioStatus::Archived {
            return Err(AutomationError::Archived);
        }
        if state.aggregate_version()? != base {
            return Err(AutomationError::Changed);
        }
        let version = state.version + 1;
        let mut write = state.write()?;
        write.routing_key_opt(routing_key);
        if name != state.name {
            write.event(&Renamed {
                name,
                renamed_by: actor.to_owned(),
            });
        }
        write.event(&Published {
            version,
            definition: definition.to_json(),
            published_by: actor.to_owned(),
        });
        write
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
            .map_err(stale)?;
        Ok(version)
    }

    /// Holds a published scenario; paused already is a no-op.
    pub async fn pause_scenario(
        &self,
        actor: &str,
        scenario_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AutomationError> {
        let state = self.require_scenario(scenario_id).await?;
        match state.status {
            ScenarioStatus::Draft => return Err(AutomationError::NotPublished),
            ScenarioStatus::Archived => return Err(AutomationError::Archived),
            ScenarioStatus::Paused => return Ok(()),
            ScenarioStatus::Active => {}
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Paused {
                paused_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
            .map_err(stale)?;
        Ok(())
    }

    /// Runs a paused scenario again; running already is a no-op.
    pub async fn resume_scenario(
        &self,
        actor: &str,
        scenario_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AutomationError> {
        let state = self.require_scenario(scenario_id).await?;
        match state.status {
            ScenarioStatus::Draft => return Err(AutomationError::NotPublished),
            ScenarioStatus::Archived => return Err(AutomationError::Archived),
            ScenarioStatus::Active => return Ok(()),
            ScenarioStatus::Paused => {}
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Resumed {
                resumed_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
            .map_err(stale)?;
        Ok(())
    }

    /// Deletes a scenario from the merchant's view; deleted already is a
    /// no-op.
    pub async fn archive_scenario(
        &self,
        actor: &str,
        scenario_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), AutomationError> {
        let state = self.require_scenario(scenario_id).await?;
        if state.status == ScenarioStatus::Archived {
            return Ok(());
        }
        state
            .write()?
            .routing_key_opt(routing_key)
            .event(&Archived {
                archived_by: actor.to_owned(),
            })
            .tenant(&self.tenant)
            .commit(&self.executor)
            .await
            .map_err(stale)?;
        Ok(())
    }
}

impl<E: Executor> super::Command<E> {
    pub async fn load_scenario(&self, scenario_id: &str) -> anyhow::Result<Option<ScenarioState>> {
        scenario_projection()
            .load(scenario_id)
            .tenant(&self.tenant)
            .execute(&self.executor)
            .await
    }

    async fn require_scenario(&self, scenario_id: &str) -> Result<ScenarioState, AutomationError> {
        self.load_scenario(scenario_id)
            .await?
            .ok_or(AutomationError::NotFound)
    }
}
