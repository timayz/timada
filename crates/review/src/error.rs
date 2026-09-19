#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("review not found")]
    ReviewNotFound,
    #[error("question not found")]
    QuestionNotFound,
    #[error("this customer already reviewed this product")]
    AlreadyReviewed,
    #[error("rating must be between 1 and 5, got {0}")]
    InvalidRating(u8),
    #[error("review is not pending moderation")]
    NotPending,
    #[error("question is not pending moderation")]
    QuestionNotPending,
    #[error("question is not published")]
    QuestionNotPublished,
    #[error("answer not found")]
    AnswerNotFound,
    #[error("answer is not pending moderation")]
    AnswerNotPending,
    #[error("this customer already answered this question")]
    AlreadyAnswered,
    #[error("`{0}` is required")]
    Required(&'static str),
    #[error(transparent)]
    Write(#[from] evento::WriteError),
    #[error(transparent)]
    Server(#[from] anyhow::Error),
}
