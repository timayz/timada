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

/// The old addresses the shop still answers, each with a permanent
/// redirect to where its content is now: one stream per shop, with the id
/// [`redirects_id`](crate::redirects_id). For a shop moved from another
/// platform, whose addresses a rename here never knew.
#[evento::aggregate(name = "timada-pages/Redirects")]
pub enum Redirects {
    /// `from` is a path with no query (`/products/blue-mug`), `to` a path
    /// of the shop (`/p/blue-mug`).
    RedirectAdded {
        redirect_id: String,
        from: String,
        to: String,
        added_by: String,
    },
    RedirectRemoved {
        redirect_id: String,
        removed_by: String,
    },
}

/// How the storefront's Google tag is set up: one stream per shop, with the
/// id [`google_tag_id`](crate::google_tag_id). The storefront writes the tag
/// into its pages from it, and holds it back as the visitor's answer to the
/// cookie banner says.
#[evento::aggregate(name = "timada-pages/GoogleTag")]
pub enum GoogleTag {
    /// The settings whole, as they stand from now on; all of them empty
    /// takes the tag off the storefront.
    GoogleTagSet {
        /// `G-…` or `GT-…`.
        analytics_id: Option<String>,
        /// `AW-…`.
        ads_id: Option<String>,
        purchase_label: Option<String>,
        add_to_cart_label: Option<String>,
        begin_checkout_label: Option<String>,
        enhanced_conversions: bool,
        set_by: String,
    },
}

/// The shop's Meta pixel: one stream per shop, with the id
/// [`meta_pixel_id`](crate::meta_pixel_id). The storefront writes the pixel
/// into its pages from the pixel's id, for the visitors who allowed
/// advertising; the host sends each sale to Meta's Conversions API as well,
/// under the token, so Meta counts it even when the browser's event is lost.
#[evento::aggregate(name = "timada-pages/MetaPixel")]
pub enum MetaPixel {
    /// The pixel the storefront writes from now on; `None` takes it off,
    /// and the token with it.
    MetaPixelSet {
        /// Digits, as Meta's Events Manager shows it.
        pixel_id: Option<String>,
        set_by: String,
    },
    /// The Conversions API token of the pixel, sealed by the host before it
    /// gets here, as the payment keys are.
    ConversionsTokenSet {
        token_sealed: Vec<u8>,
        set_by: String,
    },
    /// The token was taken off: sales are told by the browser only.
    ConversionsTokenRemoved { removed_by: String },
    /// Meta refused the sale of `order_id` sent under the token (a token
    /// revoked, a pixel it does not belong to), in Meta's words. Written by
    /// the host, once per reason; the next pixel or token set clears it.
    ConversionRefused {
        order_id: String,
        reason: String,
        refused_by: String,
    },
}
