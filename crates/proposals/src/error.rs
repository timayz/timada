use evento::WriteError;

use crate::value_object::{MAX_CHANGES, MAX_REASON_LEN, MAX_VALUE_LEN};

#[derive(Debug, thiserror::Error)]
pub enum ProposalError {
    #[error("no such kind of proposal")]
    UnknownKind,
    #[error("a proposal names what it is about")]
    InvalidSubject,
    #[error("a proposal says what prepared it")]
    InvalidSource,
    #[error("a proposal changes 1 to {MAX_CHANGES} fields of its kind, each once")]
    InvalidChanges,
    #[error("a value is at most {MAX_VALUE_LEN} characters")]
    ValueTooLong,
    #[error("the proposal changes nothing: every field already holds that value")]
    NothingToChange,
    #[error("a reason is at most {MAX_REASON_LEN} characters")]
    ReasonTooLong,
    #[error("a proposal is valid for ninety days at most")]
    InvalidValidity,
    #[error("proposal not found")]
    NotFound,
    #[error("the proposal was answered already")]
    NotWaiting,
    #[error("the proposal is no longer valid: it waited past its date")]
    Expired,
    #[error("the proposal has no such field")]
    UnknownField,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ProposalError {
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

    /// Whether the store failed rather than the request: a pass that meets
    /// it stops and tries again.
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
            Self::UnknownKind => "proposal_unknown_kind",
            Self::InvalidSubject => "proposal_invalid_subject",
            Self::InvalidSource => "proposal_invalid_source",
            Self::InvalidChanges => "proposal_invalid_changes",
            Self::ValueTooLong => "proposal_value_too_long",
            Self::NothingToChange => "proposal_nothing_to_change",
            Self::ReasonTooLong => "proposal_reason_too_long",
            Self::InvalidValidity => "proposal_invalid_validity",
            Self::NotFound => "proposal_not_found",
            Self::NotWaiting => "proposal_not_waiting",
            Self::Expired => "proposal_expired",
            Self::UnknownField => "proposal_unknown_field",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
