use evento::WriteError;

use crate::value_object::{MAX_BRIEF_LEN, MAX_KEYWORD_LEN};

#[derive(Debug, thiserror::Error)]
pub enum WriterError {
    #[error("a draft is of a product's texts or a category's")]
    UnknownKind,
    #[error("a draft names what it is about")]
    InvalidSubject,
    #[error("a keyword is at most {MAX_KEYWORD_LEN} characters")]
    InvalidKeyword,
    #[error("the tone, and what never to claim, are at most {MAX_BRIEF_LEN} characters each")]
    BriefTooLong,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl WriterError {
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
            Self::UnknownKind => "writer_unknown_kind",
            Self::InvalidSubject => "writer_invalid_subject",
            Self::InvalidKeyword => "writer_invalid_keyword",
            Self::BriefTooLong => "writer_brief_too_long",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
