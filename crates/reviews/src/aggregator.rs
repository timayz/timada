//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

use crate::value_object::ReviewPhoto;

/// A shopper's review of one product of one order. Its id is
/// [`review_id`](crate::review_id) of the two.
#[evento::aggregate(name = "timada-reviews/Review")]
pub enum Review {
    /// Waits for the merchant. `variant_label` and `author_name` come from
    /// the order, never from the shopper.
    Submitted {
        order_id: String,
        product_id: String,
        /// The variant bought (`Beige / Large`), if the product has variants.
        variant_label: Option<String>,
        /// 1 to 5 stars.
        rating: u8,
        title: Option<String>,
        body: String,
        /// How the review is signed: the first name and the initial of the
        /// last one (`Ada L.`).
        author_name: String,
        photos: Vec<ReviewPhoto>,
        submitted_by: String,
    },
    /// Shown on the shop, and counted in the product's rating.
    Published { published_by: String },
    /// Final: hidden for good, whether it was waiting or published. The
    /// reason is shown to its author.
    Rejected { reason: String, rejected_by: String },
    /// One photo taken off; the rest of the review stays as it is.
    PhotoRemoved {
        image_id: String,
        removed_by: String,
    },
    /// Its author changed it while it waited: the rating, title and text
    /// are replaced, the photos in `removed` taken off and those in `added`
    /// put after the ones kept.
    Revised {
        rating: u8,
        title: Option<String>,
        body: String,
        added: Vec<ReviewPhoto>,
        removed: Vec<String>,
        revised_by: String,
    },
    /// The merchant's public answer, shown under the review; a later one
    /// replaces it.
    Replied { body: String, replied_by: String },
    /// The merchant's answer taken off.
    ReplyRemoved { removed_by: String },
}

/// Whether the shop mails its buyers a link to review what they bought, and
/// how long after the order is fulfilled. One stream per tenant store, with
/// the id [`review_requests_id`](crate::review_requests_id); no stream means
/// the default ([`DEFAULT_REQUEST_DELAY_DAYS`](crate::DEFAULT_REQUEST_DELAY_DAYS)).
#[evento::aggregate(name = "timada-reviews/ReviewRequests")]
pub enum ReviewRequests {
    /// On, `days` after fulfilment.
    RequestDelaySet {
        days: u8,
        set_by: String,
    },
    RequestsDisabled {
        disabled_by: String,
    },
}

/// The review request mailed for one order. One stream per order, with the
/// id [`review_request_id`](crate::review_request_id): the node that writes
/// it first is the one that sends, so a buyer gets one mail however many
/// nodes follow the tenant.
#[evento::aggregate(name = "timada-reviews/ReviewRequest")]
pub enum ReviewRequest {
    RequestMailed {
        order_id: String,
        email: String,
        mailed_by: String,
    },
}

/// A mailed link that opens an order's review forms without the browser that
/// placed it. Its id is [`review_link_id`](crate::review_link_id) of the
/// token: the store never holds the token itself.
#[evento::aggregate(name = "timada-reviews/ReviewLink")]
pub enum ReviewLink {
    LinkIssued {
        order_id: String,
        /// Unix seconds.
        expires_at: u64,
        issued_by: String,
    },
}
