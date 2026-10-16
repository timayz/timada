use evento::WriteError;

use crate::value_object::{
    MAX_BODY_LEN, MAX_PHOTOS, MAX_REASON_LEN, MAX_REQUEST_DELAY_DAYS, MAX_TITLE_LEN, MIN_BODY_LEN,
};

#[derive(Debug, thiserror::Error)]
pub enum ReviewError {
    #[error("a rating is 1 to 5 stars")]
    InvalidRating,
    #[error("a title is at most {MAX_TITLE_LEN} characters")]
    InvalidTitle,
    #[error("a review says something in {MIN_BODY_LEN} to {MAX_BODY_LEN} characters")]
    InvalidBody,
    #[error("a review has at most {MAX_PHOTOS} photos")]
    TooManyPhotos,
    #[error("a reason is at most {MAX_REASON_LEN} characters")]
    InvalidReason,
    /// No paid order has this id.
    #[error("order not found")]
    OrderNotFound,
    /// The order holds no such product.
    #[error("this product was not bought with this order")]
    NotPurchased,
    #[error("this order was cancelled")]
    OrderCancelled,
    #[error("this product has already been reviewed for this order")]
    AlreadyReviewed,
    #[error("review not found")]
    NotFound,
    #[error("this review was rejected")]
    Rejected,
    #[error("this review has no such photo")]
    PhotoNotFound,
    /// Its author may change it only until the merchant publishes or
    /// rejects it.
    #[error("this review no longer waits for the shop")]
    NotPending,
    #[error("a shop asks for reviews at most {MAX_REQUEST_DELAY_DAYS} days after fulfilment")]
    RequestDelayTooLong,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ReviewError {
    /// Whether the message can be shown to the person who made the request
    /// (as opposed to an infrastructure failure).
    pub fn is_user_facing(&self) -> bool {
        !matches!(self, Self::Write(_) | Self::Other(_))
    }

    /// A stable code for the refusal, for a page to carry in a URL and word
    /// in its own language; `None` for an infrastructure failure.
    pub fn code(&self) -> Option<&'static str> {
        Some(match self {
            Self::InvalidRating => REVIEW_INVALID_RATING,
            Self::InvalidTitle => REVIEW_INVALID_TITLE,
            Self::InvalidBody => REVIEW_INVALID_BODY,
            Self::TooManyPhotos => REVIEW_TOO_MANY_PHOTOS,
            Self::InvalidReason => "invalid_review_reason",
            Self::OrderNotFound => REVIEW_ORDER_NOT_FOUND,
            Self::NotPurchased => REVIEW_NOT_PURCHASED,
            Self::OrderCancelled => REVIEW_ORDER_CANCELLED,
            Self::AlreadyReviewed => REVIEW_ALREADY_REVIEWED,
            Self::NotFound => REVIEW_NOT_FOUND,
            Self::Rejected => "review_rejected",
            Self::PhotoNotFound => REVIEW_PHOTO_NOT_FOUND,
            Self::NotPending => REVIEW_NOT_PENDING,
            Self::RequestDelayTooLong => "review_request_delay_too_long",
            Self::Write(_) | Self::Other(_) => return None,
        })
    }
}

/// The codes a shopper may meet, which a refused submission carries.
pub const REVIEW_INVALID_RATING: &str = "review_invalid_rating";
pub const REVIEW_INVALID_TITLE: &str = "review_invalid_title";
pub const REVIEW_INVALID_BODY: &str = "review_invalid_body";
pub const REVIEW_TOO_MANY_PHOTOS: &str = "review_too_many_photos";
pub const REVIEW_ORDER_NOT_FOUND: &str = "review_order_not_found";
pub const REVIEW_NOT_PURCHASED: &str = "review_not_purchased";
pub const REVIEW_ORDER_CANCELLED: &str = "review_order_cancelled";
pub const REVIEW_ALREADY_REVIEWED: &str = "review_already_reviewed";
pub const REVIEW_NOT_FOUND: &str = "review_not_found";
pub const REVIEW_PHOTO_NOT_FOUND: &str = "review_photo_not_found";
pub const REVIEW_NOT_PENDING: &str = "review_not_pending";
/// Not a domain refusal: what the upload endpoint answers for a file that is
/// not a picture it takes, or one too large.
pub const REVIEW_INVALID_PHOTO: &str = "review_invalid_photo";

/// Every code above, so one read back from an answer maps to a
/// `&'static str` again.
pub const SHOPPER_CODES: &[&str] = &[
    REVIEW_INVALID_RATING,
    REVIEW_INVALID_TITLE,
    REVIEW_INVALID_BODY,
    REVIEW_TOO_MANY_PHOTOS,
    REVIEW_ORDER_NOT_FOUND,
    REVIEW_NOT_PURCHASED,
    REVIEW_ORDER_CANCELLED,
    REVIEW_ALREADY_REVIEWED,
    REVIEW_NOT_FOUND,
    REVIEW_PHOTO_NOT_FOUND,
    REVIEW_NOT_PENDING,
    REVIEW_INVALID_PHOTO,
];
