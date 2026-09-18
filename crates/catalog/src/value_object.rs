use bitcode::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct Brand {
    pub name: String,
    pub slug: String,
}

/// One row of the "fiche technique", grouped by section.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct Spec {
    pub group: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub enum MediaKind {
    #[default]
    Image,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct Media {
    pub url: String,
    pub kind: MediaKind,
    pub alt: String,
}

/// EU energy efficiency class shown on the product page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum EnergyClass {
    A,
    B,
    C,
    D,
    #[default]
    E,
    F,
    G,
}
