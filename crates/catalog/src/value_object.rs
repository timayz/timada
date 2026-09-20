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

/// Which line of the technical sheet: `Dalle` › `Taille`. The same label may
/// live in two groups.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Encode, Decode)]
pub struct SpecKey {
    pub group: String,
    pub label: String,
}

impl SpecKey {
    pub fn new(group: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            group: group.into(),
            label: label.into(),
        }
    }
}

impl Spec {
    pub fn key(&self) -> SpecKey {
        SpecKey::new(&self.group, &self.label)
    }
}

/// What tells the variants of a family apart — `Couleur` — and the values it
/// takes, in the order shoppers are offered them: `S`, `M`, `L`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct FamilyOption {
    pub name: String,
    pub values: Vec<String>,
}

impl FamilyOption {
    pub fn new(name: impl Into<String>, values: &[&str]) -> Self {
        Self {
            name: name.into(),
            values: values.iter().map(|value| (*value).to_owned()).collect(),
        }
    }
}

/// Where a variant stands on one option: `Couleur` = `Noir`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default, Encode, Decode)]
pub struct OptionValue {
    pub option: String,
    pub value: String,
}

impl OptionValue {
    pub fn new(option: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            option: option.into(),
            value: value.into(),
        }
    }
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
