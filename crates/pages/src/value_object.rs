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
/// The picture formats a page takes, as `(extension, type)`.
pub const IMAGE_FORMATS: &[(&str, &str)] = &[
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("webp", "image/webp"),
    ("gif", "image/gif"),
];

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

/// The type of a picture format the page takes, by extension.
pub fn image_mime(ext: &str) -> Option<&'static str> {
    IMAGE_FORMATS
        .iter()
        .find(|(known, _)| *known == ext)
        .map(|(_, mime)| *mime)
}

/// Whether `file` is the name a page's picture is kept under:
/// `{ULID}.{ext}` with an extension a page takes.
pub fn is_image_file(file: &str) -> bool {
    let Some((id, ext)) = file.split_once('.') else {
        return false;
    };
    id.len() == 26
        && id
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        && image_mime(ext).is_some()
}
