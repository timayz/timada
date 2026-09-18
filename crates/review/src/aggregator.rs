use crate::value_object::AnswerAuthor;

// Explicit names pin the on-disk identity: renaming the crate or an enum must
// never orphan stored events.
#[evento::aggregate(name = "timada-review/Review")]
pub enum Review {
    /// A customer left a review on a product (one per customer per product).
    ReviewSubmitted {
        product_id: String,
        customer_id: String,
        order_id: Option<String>,
        rating: u8,
        title: String,
        body: String,
    },

    /// Moderation accepted the review; it now counts in the product rating.
    ReviewPublished,

    /// Moderation refused the review.
    ReviewRejected { reason: String },
}

#[evento::aggregate(name = "timada-review/Question")]
pub enum Question {
    /// A customer asked a question on a product page.
    QuestionAsked {
        product_id: String,
        customer_id: String,
        body: String,
    },

    /// Staff or another customer answered.
    QuestionAnswered { author: AnswerAuthor, body: String },
}
