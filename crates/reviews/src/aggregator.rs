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
}
