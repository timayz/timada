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
    /// Listed in the storefront's footer, lowest position first.
    ShownInFooter {
        position: u32,
        shown_by: String,
    },
    HiddenFromFooter {
        hidden_by: String,
    },
    /// A picture uploaded for the body, kept as `{image_id}.{ext}`.
    ImageAdded {
        image_id: String,
        /// Lowercase, no dot: `jpg`, `png`, `webp`, `gif`.
        ext: String,
        mime: String,
        /// In bytes.
        size: u64,
        width: Option<u32>,
        height: Option<u32>,
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
    Claimed { page_id: String },
    Released,
}
