use bitcode::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum ReviewStatus {
    #[default]
    Pending,
    Published,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum AnswerAuthor {
    #[default]
    Staff,
    Customer {
        customer_id: String,
    },
}
