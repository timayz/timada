use base64::{
    Engine, alphabet,
    engine::{GeneralPurpose, general_purpose},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::ops::Deref;

#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    Asc,
    Desc,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Edge<N> {
    pub cursor: Value,
    pub node: N,
}

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub has_previous_page: bool,
    pub has_next_page: bool,
    pub start_cursor: Option<Value>,
    pub end_cursor: Option<Value>,
}

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadResult<N> {
    pub edges: Vec<Edge<N>>,
    pub page_info: PageInfo,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Value(String);

impl Deref for Value {
    type Target = String;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl AsRef<[u8]> for Value {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

pub trait Cursor {
    type T: Serialize + DeserializeOwned;

    fn serialize(&self) -> Self::T;

    fn serialize_cursor(&self) -> Result<Value, ciborium::ser::Error<std::io::Error>> {
        let cursor = self.serialize();

        let mut cbor_encoded = vec![];
        ciborium::into_writer(&cursor, &mut cbor_encoded)?;

        let engine = GeneralPurpose::new(&alphabet::URL_SAFE, general_purpose::PAD);

        Ok(Value(engine.encode(cbor_encoded)))
    }

    fn deserialize_cursor(value: &Value) -> Result<Self::T, ciborium::de::Error<std::io::Error>> {
        let engine = GeneralPurpose::new(&alphabet::URL_SAFE, general_purpose::PAD);
        let decoded = engine
            .decode(value)
            .map_err(|e| ciborium::de::Error::Semantic(None, e.to_string()))?;

        ciborium::from_reader(&decoded[..])
    }
}

#[derive(Default, Serialize, Deserialize, Clone)]
pub struct Args {
    pub first: Option<u16>,
    pub after: Option<Value>,
    pub last: Option<u16>,
    pub before: Option<Value>,
}

impl Args {
    pub fn forward(first: u16, after: Option<Value>) -> Self {
        Self {
            first: Some(first),
            after,
            last: None,
            before: None,
        }
    }

    pub fn backward(last: u16, before: Option<Value>) -> Self {
        Self {
            first: None,
            after: None,
            last: Some(last),
            before,
        }
    }

    pub fn is_backward(&self) -> bool {
        (self.last.is_some() || self.before.is_some())
            && self.first.is_none()
            && self.after.is_none()
    }

    pub fn get_info(&self) -> (u16, Option<Value>) {
        if self.is_backward() {
            (self.last.unwrap_or(40), self.before.clone())
        } else {
            (self.first.unwrap_or(40), self.after.clone())
        }
    }
}
