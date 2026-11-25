use evento::WriteError;

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("a target is an amount above zero, in a currency of three letters")]
    InvalidTarget,
    #[error("a target's day reads 2027-06-30, and is yet to come")]
    InvalidTargetDay,
    #[error("a threshold is a whole number, or an amount like 0.70, within its range")]
    InvalidThreshold,
    #[error("no such checkpoint")]
    UnknownCheckpoint,
    #[error("the checkpoint's day has not come")]
    NotReached,
    #[error("a decision is to go on, to fix or to stop")]
    InvalidDecision,
    #[error("a note is at most 1,000 characters")]
    NoteTooLong,
    #[error("no such step to confirm")]
    UnknownStep,
    #[error("a number of products is between 1 and 100,000")]
    InvalidPathTarget,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl LaunchError {
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

    /// Whether the store failed rather than the request.
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
            Self::InvalidTarget => "launch_invalid_target",
            Self::InvalidTargetDay => "launch_invalid_target_day",
            Self::InvalidThreshold => "launch_invalid_threshold",
            Self::UnknownCheckpoint => "launch_unknown_checkpoint",
            Self::NotReached => "launch_not_reached",
            Self::InvalidDecision => "launch_invalid_decision",
            Self::NoteTooLong => "launch_note_too_long",
            Self::UnknownStep => "launch_unknown_step",
            Self::InvalidPathTarget => "launch_invalid_path_target",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
