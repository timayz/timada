//! The file a catalogue is seeded from: what the catalog needs, nothing of
//! where it came from (not WooCommerce's shape, not the dashboard's).

use std::io::{Read, Write};
use std::path::Path;

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// A catalogue to load: categories first (a parent before its children is
/// not required, the import orders them), then products.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Where it was taken from, for the record.
    #[serde(default)]
    pub source: String,
    /// Unix seconds.
    #[serde(default)]
    pub fetched_at: u64,
    #[serde(default)]
    pub categories: Vec<CategorySeed>,
    #[serde(default)]
    pub products: Vec<ProductSeed>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategorySeed {
    pub slug: String,
    pub name: String,
    /// Plain text.
    #[serde(default)]
    pub description: String,
    /// The parent's slug; `None` at the top.
    #[serde(default)]
    pub parent: Option<String>,
    /// Among its siblings, lowest first.
    #[serde(default)]
    pub position: u32,
    /// Shown in the shop's menu.
    #[serde(default = "yes")]
    pub in_menu: bool,
    /// A picture for the category, which the catalog has no place for yet;
    /// kept so a storefront may use it.
    #[serde(default)]
    pub image: Option<ImageSeed>,
}

fn yes() -> bool {
    true
}

/// In the menu, like a category made in the back office.
impl Default for CategorySeed {
    fn default() -> Self {
        Self {
            slug: String::new(),
            name: String::new(),
            description: String::new(),
            parent: None,
            position: 0,
            in_menu: true,
            image: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductSeed {
    pub slug: String,
    pub name: String,
    /// Plain text.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub sku: Option<String>,
    pub currency: String,
    /// The product's own price; `None` when it sells through its variants.
    #[serde(default)]
    pub amount_cents: Option<u64>,
    /// Category slugs, the first being the one a breadcrumb follows.
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub images: Vec<ImageSeed>,
    #[serde(default)]
    pub options: Vec<OptionSeed>,
    #[serde(default)]
    pub variants: Vec<VariantSeed>,
    #[serde(default)]
    pub attributes: Vec<AttributeSeed>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageSeed {
    pub url: String,
    #[serde(default)]
    pub alt: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OptionSeed {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantSeed {
    /// One per option of the product.
    pub selections: Vec<SelectionSeed>,
    #[serde(default)]
    pub sku: Option<String>,
    pub amount_cents: u64,
    /// Index into the product's `images`.
    #[serde(default)]
    pub image: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectionSeed {
    /// The option's name.
    pub option: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeSeed {
    pub name: String,
    pub value: String,
}

impl Snapshot {
    /// Reads a snapshot, gunzipping it when the file name ends in `.gz`.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let bytes =
            std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
        let json = if is_gz(path) {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .read_to_end(&mut out)
                .with_context(|| format!("{} is not gzip", path.display()))?;
            out
        } else {
            bytes
        };
        serde_json::from_slice(&json)
            .with_context(|| format!("{} is not a snapshot", path.display()))
    }

    /// Writes a snapshot, gzipped when the file name ends in `.gz`.
    pub fn write(&self, path: &Path) -> anyhow::Result<()> {
        let json = serde_json::to_vec(self)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let bytes = if is_gz(path) {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
            encoder.write_all(&json)?;
            encoder.finish()?
        } else {
            json
        };
        std::fs::write(path, bytes).with_context(|| format!("cannot write {}", path.display()))
    }
}

fn is_gz(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("gz"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_gzip_and_plain_json() {
        let snapshot = Snapshot {
            source: "test".into(),
            fetched_at: 1,
            categories: vec![CategorySeed {
                slug: "toys".into(),
                name: "Toys".into(),
                ..Default::default()
            }],
            products: vec![ProductSeed {
                slug: "ball".into(),
                name: "Ball".into(),
                currency: "EUR".into(),
                amount_cents: Some(990),
                categories: vec!["toys".into()],
                ..Default::default()
            }],
        };
        let dir = std::env::temp_dir().join(format!("timada-snapshot-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["catalog.json", "catalog.json.gz"] {
            let path = dir.join(name);
            snapshot.write(&path).unwrap();
            assert_eq!(Snapshot::read(&path).unwrap(), snapshot, "{name}");
        }
        assert!(
            std::fs::read(dir.join("catalog.json.gz"))
                .unwrap()
                .starts_with(&[0x1f, 0x8b]),
            "gzip magic"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_category_is_in_the_menu_unless_said_otherwise() {
        let seed: CategorySeed = serde_json::from_str(r#"{"slug":"a","name":"A"}"#).unwrap();
        assert!(seed.in_menu);
    }
}
