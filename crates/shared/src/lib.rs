use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Metadata {
    pub request_id: String,
    pub request_by: String,
    pub request_as: Option<String>,
}
