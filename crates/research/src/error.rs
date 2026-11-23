use evento::WriteError;

use crate::value_object::{MAX_NAME_LEN, MAX_SEED_LEN, MAX_SEEDS};

#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    #[error("a study's name is 1 to {MAX_NAME_LEN} characters")]
    InvalidName,
    #[error("a seed keyword is 1 to {MAX_SEED_LEN} characters")]
    InvalidSeed,
    #[error("a study starts from one seed keyword at least")]
    NoSeed,
    #[error("a study holds at most {MAX_SEEDS} seed keywords")]
    TooManySeeds,
    #[error("give a two-letter country and a language code")]
    InvalidPlace,
    /// Unknown, deleted, or another user's: a study is nobody else's
    /// business, so the three read the same.
    #[error("study not found")]
    NotFound,
    #[error("the study has no such seed keyword")]
    UnknownSeed,
    #[error("these keywords are being fetched")]
    Fetching,
    #[error("every seed keyword has its keywords, or is being fetched")]
    NothingToFetch,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ResearchError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        match self {
            // A write the store is not taking right now is told to the person.
            Self::Write(err) => timada_core::write_refusal(err).is_some(),
            Self::Other(_) => false,
            _ => true,
        }
    }

    /// Whether the store failed rather than the request: a saga fails its
    /// pass on it and tries again.
    pub fn is_store_fault(&self) -> bool {
        match self {
            Self::Write(err) => timada_core::is_store_write_fault(err),
            Self::Other(_) => true,
            _ => false,
        }
    }

    /// A stable code for a page to carry in a URL; `None` for an
    /// infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidName => "invalid_study_name",
            Self::InvalidSeed => "invalid_seed",
            Self::NoSeed => "study_without_seed",
            Self::TooManySeeds => "too_many_seeds",
            Self::InvalidPlace => "invalid_study_place",
            Self::NotFound => "study_not_found",
            Self::UnknownSeed => "unknown_seed",
            Self::Fetching => "keywords_fetching",
            Self::NothingToFetch => "nothing_to_fetch",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
