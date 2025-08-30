use bincode::{Decode, Encode};

#[derive(Debug, Default, Encode, Decode)]
pub struct RequestMetadata {
    pub id: String,
    pub user_id: String,
    pub user_owner_id: Option<String>,
}
