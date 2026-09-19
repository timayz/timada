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

    /// An answer published as it is written: the shop's own. Answering a
    /// question that still awaits moderation publishes it.
    QuestionAnswered { author: AnswerAuthor, body: String },

    /// Moderation let the question through: everyone can read it, and
    /// signed-in customers can answer it.
    QuestionPublished,

    /// Moderation refused the question.
    QuestionRejected { reason: String },

    /// A customer answered a published question; the answer awaits moderation.
    AnswerSubmitted {
        answer_id: String,
        customer_id: String,
        body: String,
    },

    /// Moderation let the customer's answer through.
    AnswerPublished { answer_id: String },

    /// Moderation refused the customer's answer.
    AnswerRejected { answer_id: String, reason: String },
}
