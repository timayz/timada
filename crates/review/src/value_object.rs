use bitcode::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum ReviewStatus {
    #[default]
    Pending,
    Published,
    Rejected,
}

impl ReviewStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewStatus::Pending => "pending",
            ReviewStatus::Published => "published",
            ReviewStatus::Rejected => "rejected",
        }
    }
}

/// Where a question, or a customer's answer to one, stands with moderation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModerationStatus {
    #[default]
    Pending,
    Published,
    Rejected,
}

impl ModerationStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ModerationStatus::Pending => "pending",
            ModerationStatus::Published => "published",
            ModerationStatus::Rejected => "rejected",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum AnswerAuthor {
    #[default]
    Staff,
    Customer {
        customer_id: String,
    },
}
