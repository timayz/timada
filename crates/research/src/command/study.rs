//! What the owner does to a study itself: start it, rename it, give it one
//! more seed, delete it.

use evento::Executor;

use crate::aggregator::{Created, Deleted, Renamed, SeedAdded};
use crate::error::ResearchError;
use crate::value_object::{
    MAX_SEEDS, new_study_id, normalize_country, normalize_language, normalize_name, normalize_seed,
    normalize_seeds,
};

use super::{Command, Step, StudyState};

/// A study to start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CreateStudy {
    pub name: String,
    /// One at least; blank ones are passed over.
    pub seeds: Vec<String>,
    /// Two letters (`FR`).
    pub country: String,
    /// `fr`.
    pub language: String,
}

#[evento::command]
impl<E: Executor> Command<E> {
    /// Starts a study, the actor its owner. Nothing is fetched yet: the
    /// owner asks for the keywords, each seed a paid call.
    pub async fn create_study(
        &self,
        actor_user_id: &str,
        input: CreateStudy,
        routing_key: Option<String>,
    ) -> Result<StudyState, ResearchError> {
        let name = normalize_name(&input.name)?;
        let country = normalize_country(&input.country)?;
        let language = normalize_language(&input.language)?;
        let seeds = normalize_seeds(&input.seeds)?;
        let study_id = new_study_id();
        let mut write = evento::append(&study_id);
        write
            .original_version(0)
            .routing_key_opt(routing_key)
            .event(&Created {
                name,
                country,
                language,
                created_by: actor_user_id.to_owned(),
            });
        for seed in seeds {
            write.event(&SeedAdded {
                seed,
                added_by: actor_user_id.to_owned(),
            });
        }
        write.commit(&self.0).await?;
        self.require_owned(&study_id, actor_user_id).await
    }

    /// Renames the study; the same name is a no-op.
    pub async fn rename_study(
        &self,
        actor_user_id: &str,
        study_id: &str,
        name: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let name = normalize_name(name)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.name == name {
                return Ok(Step::Leave(()));
            }
            write.event(&Renamed {
                name: name.clone(),
                renamed_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Gives the study one more seed; one it already holds is a no-op.
    /// Its keywords are fetched when the owner asks.
    pub async fn add_seed(
        &self,
        actor_user_id: &str,
        study_id: &str,
        seed: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        let seed = normalize_seed(seed)?;
        self.settle_owned(actor_user_id, study_id, routing_key, |study, write| {
            if study.seed(&seed).is_some() {
                return Ok(Step::Leave(()));
            }
            if study.seeds.len() >= MAX_SEEDS {
                return Err(ResearchError::TooManySeeds);
            }
            write.event(&SeedAdded {
                seed: seed.clone(),
                added_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }

    /// Deletes the study: it is gone for its owner, and a fetch on its way
    /// records nothing.
    pub async fn delete_study(
        &self,
        actor_user_id: &str,
        study_id: &str,
        routing_key: Option<String>,
    ) -> Result<(), ResearchError> {
        self.settle_owned(actor_user_id, study_id, routing_key, |_, write| {
            write.event(&Deleted {
                deleted_by: actor_user_id.to_owned(),
            });
            Ok(Step::Write(()))
        })
        .await
    }
}
