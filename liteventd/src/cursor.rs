use base64::{
    Engine, alphabet,
    engine::{GeneralPurpose, general_purpose},
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum Order {
    Asc,
    Desc,
}

#[derive(Debug, PartialEq, Serialize, Deserialize, Clone)]
pub struct Edge<N> {
    pub cursor: Cursor,
    pub node: N,
}

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub has_previous_page: bool,
    pub has_next_page: bool,
    pub start_cursor: Option<Cursor>,
    pub end_cursor: Option<Cursor>,
}

#[derive(Default, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReadResult<N> {
    pub edges: Vec<Edge<N>>,
    pub page_info: PageInfo,
}

#[derive(Debug, PartialEq, Deserialize, Serialize, Clone)]
pub struct Cursor(String);

impl From<String> for Cursor {
    fn from(val: String) -> Self {
        Self(val)
    }
}

impl AsRef<[u8]> for Cursor {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

pub trait ToCursor {
    type Cursor: Serialize;

    fn serialize_cursor(&self) -> Self::Cursor;
    fn to_cursor(&self) -> Result<Cursor, ciborium::ser::Error<std::io::Error>> {
        let cursor = self.serialize_cursor();

        let mut cbor_encoded = vec![];
        ciborium::into_writer(&cursor, &mut cbor_encoded)?;

        let engine = GeneralPurpose::new(&alphabet::URL_SAFE, general_purpose::PAD);

        Ok(Cursor(engine.encode(cbor_encoded)))
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Args {
    pub first: Option<u16>,
    pub after: Option<Cursor>,
    pub last: Option<u16>,
    pub before: Option<Cursor>,
}

impl Args {
    pub fn forward(first: u16, after: Option<Cursor>) -> Self {
        Self {
            first: Some(first),
            after,
            last: None,
            before: None,
        }
    }

    pub fn backward(last: u16, before: Option<Cursor>) -> Self {
        Self {
            first: None,
            after: None,
            last: Some(last),
            before,
        }
    }
}
