use evento::WriteError;

use crate::definition::Problem;
use crate::value_object::MAX_NAME_LEN;

#[derive(Debug, thiserror::Error)]
pub enum AutomationError {
    #[error("a name is 1 to {MAX_NAME_LEN} characters")]
    InvalidName,
    #[error("scenario not found")]
    NotFound,
    /// Saved by someone else since the editor loaded it: the copy is stale.
    #[error("the scenario changed since it was opened")]
    Changed,
    #[error("the scenario was deleted")]
    Archived,
    /// Paused or resumed before it was ever published.
    #[error("the scenario was never published")]
    NotPublished,
    /// What keeps it from being published.
    #[error("the scenario is not ready to publish")]
    Invalid(Vec<Problem>),
    /// Beyond the limits even for a draft, or of a newer format.
    #[error("the scenario cannot be saved as it is")]
    TooLarge(Vec<Problem>),
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AutomationError {
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

    /// A stable code for the refusal, for a page to carry in a URL and word
    /// in its own language; `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidName => SCENARIO_INVALID_NAME,
            Self::NotFound => SCENARIO_NOT_FOUND,
            Self::Changed => SCENARIO_CHANGED,
            Self::Archived => SCENARIO_ARCHIVED,
            Self::NotPublished => SCENARIO_NOT_PUBLISHED,
            Self::Invalid(_) => SCENARIO_INVALID,
            Self::TooLarge(_) => SCENARIO_TOO_LARGE,
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}

pub const SCENARIO_INVALID_NAME: &str = "scenario_invalid_name";
pub const SCENARIO_NOT_FOUND: &str = "scenario_not_found";
pub const SCENARIO_CHANGED: &str = "scenario_changed";
pub const SCENARIO_ARCHIVED: &str = "scenario_archived";
pub const SCENARIO_NOT_PUBLISHED: &str = "scenario_not_published";
pub const SCENARIO_INVALID: &str = "scenario_invalid";
pub const SCENARIO_TOO_LARGE: &str = "scenario_too_large";
