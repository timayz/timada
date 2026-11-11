//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// One page of the shop. Its id is a fresh [`new_page_id`](crate::new_page_id).
#[evento::aggregate(name = "timada-pages/Page")]
pub enum Page {
    /// Written as a draft: nobody outside the back office sees it.
    Created {
        slug: String,
        title: String,
        /// Markdown, as the merchant wrote it.
        body: String,
        created_by: String,
    },
    Edited {
        title: String,
        body: String,
        edited_by: String,
    },
    /// What search engines show for it; empty falls back to the title and
    /// the start of the body.
    SeoSet {
        meta_title: String,
        meta_description: String,
        set_by: String,
    },
    /// Moved to another slug, which [`PageSlug`] holds from then on.
    Renamed {
        slug: String,
        renamed_by: String,
    },
    Published {
        published_by: String,
    },
    /// Back to a draft.
    Unpublished {
        unpublished_by: String,
    },
    /// A picture uploaded for the body, kept in the two renditions
    /// `picture` describes ([`timada_core::Picture`]) and named in the
    /// body as `{image_id}.webp`.
    ImageAdded {
        image_id: String,
        picture: timada_core::Picture,
        added_by: String,
    },
    ImageRemoved {
        image_id: String,
        removed_by: String,
    },
    /// Gone for good: its slug is free again and its pictures deleted.
    Deleted {
        deleted_by: String,
    },
}

/// The index `slug → page`: one stream per slug, with the deterministic id
/// [`page_slug_id`](crate::page_slug_id).
#[evento::aggregate(name = "timada-pages/PageSlug")]
pub enum PageSlug {
    /// `page_version` is the version of the page the claim was made at: the
    /// page's next write moves it to the slug or fails, so a page past it
    /// has used the claim or never will.
    Claimed {
        page_id: String,
        page_version: u16,
    },
    Released,
}

/// The short messages the storefront's top bar cycles through ("-10% with
/// the code WELCOME10"): one stream per shop, with the id
/// [`announcements_id`](crate::announcements_id). Times are Unix seconds;
/// a message shows from `starts_at` (none: at once) until `ends_at` (none:
/// for as long as it is not hidden).
#[evento::aggregate(name = "timada-pages/Announcements")]
pub enum Announcements {
    /// Shown, last in the bar.
    AnnouncementAdded {
        announcement_id: String,
        /// Text with `**bold**` and at most one `[link](/path)`.
        text: String,
        starts_at: Option<u64>,
        ends_at: Option<u64>,
        added_by: String,
    },
    /// Replaces the text and the window.
    AnnouncementChanged {
        announcement_id: String,
        text: String,
        starts_at: Option<u64>,
        ends_at: Option<u64>,
        changed_by: String,
    },
    /// Kept in the back office, off the bar.
    AnnouncementHidden {
        announcement_id: String,
        hidden_by: String,
    },
    AnnouncementShown {
        announcement_id: String,
        shown_by: String,
    },
    /// Moved to `position` (0 is first) in the order the bar shows them.
    AnnouncementMoved {
        announcement_id: String,
        position: u32,
        moved_by: String,
    },
    AnnouncementRemoved {
        announcement_id: String,
        removed_by: String,
    },
}
