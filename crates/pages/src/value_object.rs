//! Identifiers, limits and the rules a page's fields follow.

use crate::error::PageError;

pub const MAX_TITLE_LEN: usize = 120;
/// The longest body, in characters: room for the longest terms of sale.
pub const MAX_BODY_LEN: usize = 100_000;
/// What search engines show of a title before cutting it.
pub const MAX_META_TITLE_LEN: usize = 70;
/// What search engines show of a description before cutting it.
pub const MAX_META_DESCRIPTION_LEN: usize = 160;
pub use timada_core::MIN_SLUG_LEN;
pub const MAX_SLUG_LEN: usize = 64;
/// The slug of a title with no usable characters.
pub const FALLBACK_SLUG: &str = "page";
/// How many pictures one page carries, at most.
pub const MAX_IMAGES_PER_PAGE: usize = 50;

/// A fresh id for a page: a ULID.
pub fn new_page_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// A fresh id for a page's picture: a ULID, unguessable, as the file it is
/// served from is public.
pub fn new_page_image_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The index stream of a page slug.
pub fn page_slug_id(slug: &str) -> String {
    evento::hash_ids(vec!["page-slug", slug])
}

/// Trims a title and checks it is 1 to 120 characters.
pub fn normalize_title(title: &str) -> Result<String, PageError> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > MAX_TITLE_LEN {
        return Err(PageError::InvalidTitle);
    }
    Ok(title.to_owned())
}

/// Checks a body is at most 100 000 characters, line breaks as `\n`
/// (browsers send a textarea's as `\r\n`). Trailing blanks are dropped;
/// empty is fine, for a page still to be written.
pub fn normalize_body(body: &str) -> Result<String, PageError> {
    let body = body.replace("\r\n", "\n");
    let body = body.trim_end();
    if body.chars().count() > MAX_BODY_LEN {
        return Err(PageError::InvalidBody);
    }
    Ok(body.to_owned())
}

/// Trims the search engines' title and description and checks their length.
/// Empty is fine: the page's own title and text stand in.
pub fn normalize_seo(
    meta_title: &str,
    meta_description: &str,
) -> Result<(String, String), PageError> {
    let meta_title = meta_title.trim();
    let meta_description = meta_description
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if meta_title.chars().count() > MAX_META_TITLE_LEN
        || meta_description.chars().count() > MAX_META_DESCRIPTION_LEN
    {
        return Err(PageError::InvalidSeo);
    }
    Ok((meta_title.to_owned(), meta_description))
}

/// Lowercases and validates a slug: `[a-z0-9]`, `-` inside, 2 to 64 characters.
pub fn normalize_slug(slug: &str) -> Result<String, PageError> {
    timada_core::normalize_slug(slug, MAX_SLUG_LEN).ok_or(PageError::InvalidSlug)
}

/// The slug a title gives: ASCII-transliterated, lowercased, runs of anything
/// else collapsed into one dash, cut to [`MAX_SLUG_LEN`]. A title with nothing
/// usable gives [`FALLBACK_SLUG`].
pub fn slugify(title: &str) -> String {
    timada_core::slugify(title, MAX_SLUG_LEN, FALLBACK_SLUG)
}

/// Whether `file` is the name a page's body knows a picture by:
/// `{ULID}.webp` ([`timada_core::is_picture_file`]).
pub fn is_image_file(file: &str) -> bool {
    timada_core::is_picture_file(file)
}

/// How many messages the top bar holds, shown or hidden.
pub const MAX_ANNOUNCEMENTS: usize = 10;
/// The longest message, marks included: one line on a phone, two at most.
pub const MAX_ANNOUNCEMENT_LEN: usize = 200;

/// The stream of the shop's announcements: one per tenant store.
pub fn announcements_id() -> String {
    evento::hash_ids(vec!["announcements"])
}

/// A fresh id for an announcement: a ULID.
pub fn new_announcement_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// Puts a message on one line, checks it is 1 to 200 characters with
/// marks it may carry, and that its window, when it has both ends, ends
/// after it starts.
pub fn normalize_announcement(
    text: &str,
    starts_at: Option<u64>,
    ends_at: Option<u64>,
) -> Result<String, PageError> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.chars().count() > MAX_ANNOUNCEMENT_LEN {
        return Err(PageError::InvalidAnnouncement);
    }
    crate::announcement::check_announcement_marks(&text)?;
    if let (Some(starts_at), Some(ends_at)) = (starts_at, ends_at)
        && ends_at <= starts_at
    {
        return Err(PageError::InvalidAnnouncementWindow);
    }
    Ok(text)
}
