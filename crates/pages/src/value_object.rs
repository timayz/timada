//! Identifiers, limits and the rules a page's fields follow.

use crate::error::PageError;

pub const MAX_TITLE_LEN: usize = 120;
/// The longest body, in characters: room for the longest terms of sale.
pub const MAX_BODY_LEN: usize = 100_000;
pub use timada_core::{MAX_META_DESCRIPTION_LEN, MAX_META_TITLE_LEN, MIN_SLUG_LEN};
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
    timada_core::normalize_seo(meta_title, meta_description).ok_or(PageError::InvalidSeo)
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

/// The longest summary of an article: two sentences.
pub const MAX_SUMMARY_LEN: usize = 300;
/// The slug of an article whose title has no usable characters.
pub const FALLBACK_ARTICLE_SLUG: &str = "article";

/// A fresh id for an article: a ULID.
pub fn new_article_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// The index stream of an article slug: the blog's slugs are its own, so
/// an article and a page may share one.
pub fn article_slug_id(slug: &str) -> String {
    evento::hash_ids(vec!["article-slug", slug])
}

/// The slug an article's title gives, as [`slugify`] does a page's; a title
/// with nothing usable gives [`FALLBACK_ARTICLE_SLUG`].
pub fn article_slug(title: &str) -> String {
    timada_core::slugify(title, MAX_SLUG_LEN, FALLBACK_ARTICLE_SLUG)
}

/// Puts a summary on one line and checks it is at most
/// [`MAX_SUMMARY_LEN`] characters. Empty is fine: the start of the body
/// stands in.
pub fn normalize_summary(summary: &str) -> Result<String, PageError> {
    let summary = summary.split_whitespace().collect::<Vec<_>>().join(" ");
    if summary.chars().count() > MAX_SUMMARY_LEN {
        return Err(PageError::InvalidSummary);
    }
    Ok(summary)
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

/// How many redirects a shop keeps: a short list written by hand.
pub const MAX_REDIRECTS: usize = 500;
/// The longest path of a redirect, either side.
pub const MAX_REDIRECT_PATH_LEN: usize = 300;

/// The stream of the shop's redirects: one per tenant store.
pub fn redirects_id() -> String {
    evento::hash_ids(vec!["redirects"])
}

/// A fresh id for a redirect: a ULID.
pub fn new_redirect_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// Whether `path` is a path of the shop: `/…`, not `//host`, with no blank
/// and nothing a header or an attribute would trip on.
fn is_shop_path(path: &str) -> bool {
    path.starts_with('/')
        && !path.starts_with("//")
        && path.chars().count() <= MAX_REDIRECT_PATH_LEN
        && !path
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '"' | '<' | '>' | '\\'))
}

/// The old path of a redirect: a path of the shop with no query and no
/// fragment, its trailing slash dropped, written as a request carries it:
/// in ASCII, anything else percent-encoded (`/caf%C3%A9`), since it is
/// compared with the request's path as it comes. The home is refused: it
/// answers.
pub fn normalize_redirect_from(from: &str) -> Result<String, PageError> {
    let from = from.trim();
    if !is_shop_path(from) || !from.is_ascii() || from.contains(['?', '#']) {
        return Err(PageError::InvalidRedirect);
    }
    let from = from.trim_end_matches('/');
    if from.is_empty() {
        return Err(PageError::InvalidRedirect);
    }
    Ok(from.to_owned())
}

/// Where a redirect sends: a path of the shop, which may carry a query.
pub fn normalize_redirect_to(to: &str) -> Result<String, PageError> {
    let to = to.trim();
    if !is_shop_path(to) {
        return Err(PageError::InvalidRedirect);
    }
    Ok(to.to_owned())
}

/// The longest Google tag id or conversion label.
pub const MAX_GOOGLE_ID_LEN: usize = 40;

/// The stream of the shop's Google tag: one per tenant store.
pub fn google_tag_id() -> String {
    evento::hash_ids(vec!["google-tag"])
}

/// `text` trimmed, when something is left.
fn given(text: Option<&str>) -> Option<&str> {
    text.map(str::trim).filter(|text| !text.is_empty())
}

/// Whether `id` is `<prefix>-` then letters and digits. The ids go into
/// the storefront's scripts, so nothing else passes.
fn is_google_id(id: &str, prefixes: &[&str]) -> bool {
    id.len() <= MAX_GOOGLE_ID_LEN
        && prefixes.iter().any(|prefix| {
            id.strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('-'))
                .is_some_and(|rest| {
                    rest.len() >= 4 && rest.bytes().all(|b| b.is_ascii_alphanumeric())
                })
        })
}

/// A Google Analytics measurement id (`G-XXXXXXXXXX`) or a combined Google
/// tag's (`GT-…`), uppercase; nothing when left empty.
pub fn normalize_analytics_id(id: Option<&str>) -> Result<Option<String>, PageError> {
    let Some(id) = given(id) else {
        return Ok(None);
    };
    let id = id.to_ascii_uppercase();
    if is_google_id(&id, &["G", "GT"]) {
        Ok(Some(id))
    } else {
        Err(PageError::InvalidGoogleTagId)
    }
}

/// A Google Ads conversion id (`AW-123456789`); nothing when left empty.
pub fn normalize_ads_id(id: Option<&str>) -> Result<Option<String>, PageError> {
    let Some(id) = given(id) else {
        return Ok(None);
    };
    let id = id.to_ascii_uppercase();
    let digits = id
        .strip_prefix("AW-")
        .is_some_and(|rest| rest.bytes().all(|b| b.is_ascii_digit()));
    if digits && is_google_id(&id, &["AW"]) {
        Ok(Some(id))
    } else {
        Err(PageError::InvalidGoogleTagId)
    }
}

/// The label of a Google Ads conversion action, the part after the slash
/// of its `AW-123456789/AbC-D_efG`: letters, digits, dashes and
/// underscores, kept as typed. The whole `AW-…/label` pasted is taken for
/// its label. Nothing when left empty.
pub fn normalize_conversion_label(label: Option<&str>) -> Result<Option<String>, PageError> {
    let Some(label) = given(label) else {
        return Ok(None);
    };
    let label = label.rsplit_once('/').map_or(label, |(_, label)| label);
    let valid = !label.is_empty()
        && label.len() <= MAX_GOOGLE_ID_LEN
        && label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'));
    if valid {
        Ok(Some(label.to_owned()))
    } else {
        Err(PageError::InvalidConversionLabel)
    }
}

/// The longest title of the newsletter pop-up.
pub const MAX_POPUP_TITLE_LEN: usize = 80;
/// The longest text under it.
pub const MAX_POPUP_TEXT_LEN: usize = 300;
/// The longest a pop-up waits on a page before opening, in seconds.
pub const MAX_POPUP_DELAY_SECS: u32 = 600;

/// The stream of the shop's newsletter pop-up: one per tenant store.
pub fn newsletter_popup_id() -> String {
    evento::hash_ids(vec!["newsletter-popup"])
}

/// How many languages a storefront is read in beside its main one.
pub const MAX_OTHER_LANGUAGES: usize = 10;

/// The stream of the shop's languages: one per tenant store.
pub fn languages_id() -> String {
    evento::hash_ids(vec!["languages"])
}

/// What a translation words: one of the shop's pages or an article of its
/// blog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextKind {
    Page,
    Article,
}

impl TextKind {
    /// The name the events and the index ids carry.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Article => "article",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "page" => Some(Self::Page),
            "article" => Some(Self::Article),
            _ => None,
        }
    }
}

/// The stream of what a page or an article says in `language`.
pub fn text_translation_id(kind: TextKind, subject_id: &str, language: &str) -> String {
    evento::hash_ids(vec!["translation", kind.as_str(), subject_id, language])
}

/// The index stream of an article's slug in `language`.
pub fn text_translation_slug_id(language: &str, slug: &str) -> String {
    evento::hash_ids(vec!["translation-slug", "article", language, slug])
}

/// The stream of the languages an article slug has a translation entry in.
pub fn slug_languages_id(slug: &str) -> String {
    evento::hash_ids(vec!["slug-languages", "article", slug])
}

/// The longest Meta pixel id.
pub const MAX_META_PIXEL_ID_LEN: usize = 20;

/// How long of Meta's refusal the pixel's stream keeps.
pub const MAX_META_REASON_LEN: usize = 200;

/// The stream of the shop's Meta pixel: one per tenant store.
pub fn meta_pixel_id() -> String {
    evento::hash_ids(vec!["meta-pixel"])
}

/// A Meta pixel id: 5 to [`MAX_META_PIXEL_ID_LEN`] digits (it is written
/// into the storefront's scripts, so nothing else passes); nothing when
/// left empty.
pub fn normalize_meta_pixel_id(id: Option<&str>) -> Result<Option<String>, PageError> {
    let Some(id) = given(id) else {
        return Ok(None);
    };
    let valid =
        (5..=MAX_META_PIXEL_ID_LEN).contains(&id.len()) && id.bytes().all(|b| b.is_ascii_digit());
    if valid {
        Ok(Some(id.to_owned()))
    } else {
        Err(PageError::InvalidMetaPixelId)
    }
}
