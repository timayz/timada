use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RequestMetadata {
    pub id: String,
    pub user_id: String,
    pub user_owner_id: Option<String>,
}
