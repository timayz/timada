use evento::WriteError;

use crate::value_object::{
    MAX_BODY_LEN, MAX_IMAGES_PER_PAGE, MAX_META_DESCRIPTION_LEN, MAX_META_TITLE_LEN, MAX_TITLE_LEN,
};

#[derive(Debug, thiserror::Error)]
pub enum PagesError {
    #[error("a title is 1 to {MAX_TITLE_LEN} characters")]
    InvalidTitle,
    #[error("a page is at most {MAX_BODY_LEN} characters")]
    InvalidBody,
    #[error(
        "the search engines' title is at most {MAX_META_TITLE_LEN} characters, the description {MAX_META_DESCRIPTION_LEN}"
    )]
    InvalidSeo,
    #[error("a slug is 2 to 64 lowercase letters, digits or dashes")]
    InvalidSlug,
    #[error("another page has this slug")]
    SlugTaken,
    #[error("a page carries at most {MAX_IMAGES_PER_PAGE} pictures")]
    TooManyImages,
    #[error("a page takes JPEG, PNG, WebP or GIF pictures")]
    UnsupportedImage,
    /// Unknown, or deleted.
    #[error("page not found")]
    NotFound,
    #[error("picture not found")]
    ImageNotFound,
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl PagesError {
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
            Self::InvalidTitle => "page_invalid_title",
            Self::InvalidBody => "page_invalid_body",
            Self::InvalidSeo => "page_invalid_seo",
            Self::InvalidSlug => "page_invalid_slug",
            Self::SlugTaken => "page_slug_taken",
            Self::TooManyImages => "page_too_many_images",
            Self::UnsupportedImage => "unsupported_image",
            Self::NotFound => "page_not_found",
            Self::ImageNotFound => "page_image_not_found",
            Self::Write(err) => return timada_core::write_refusal(err),
            Self::Other(_) => return None,
        })
    }
}
