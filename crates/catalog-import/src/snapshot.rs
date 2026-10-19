//! The files a catalogue is seeded from: what the catalog needs, nothing of
//! where it came from (not WooCommerce's shape, not the dashboard's). A
//! snapshot is a directory holding `catalog.json.gz` and, under `pictures/`,
//! the pictures the catalogue names; a picture the directory lacks is
//! fetched from its address when the catalogue is loaded.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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
    /// The directory the pictures' [`ImageSeed::file`]s are relative to:
    /// where the snapshot was read from. Not part of the file.
    #[serde(skip)]
    pub dir: PathBuf,
}

/// The catalogue file inside a snapshot directory.
pub const CATALOG_FILE: &str = "catalog.json.gz";
/// The pictures' directory inside a snapshot directory.
pub const PICTURES_DIR: &str = "pictures";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CategorySeed {
    pub slug: String,
    pub name: String,
    /// Plain text.
    #[serde(default)]
    pub description: String,
    /// Plain text, shown below the category's products.
    #[serde(default)]
    pub body: String,
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
            body: String::new(),
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
    /// Where the shop serves it.
    pub url: String,
    #[serde(default)]
    pub alt: String,
    /// Its bytes in the snapshot, as a path relative to the snapshot's
    /// directory (`pictures/<hash>.jpg`); fetched from `url` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
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
    /// Reads a snapshot: a directory holding [`CATALOG_FILE`], or a
    /// catalogue file itself (gunzipped when its name ends in `.gz`), whose
    /// pictures then resolve from its directory.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let path = if path.is_dir() {
            path.join(CATALOG_FILE)
        } else {
            path.to_path_buf()
        };
        let bytes =
            std::fs::read(&path).with_context(|| format!("cannot read {}", path.display()))?;
        let json = if is_gz(&path) {
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(bytes.as_slice())
                .read_to_end(&mut out)
                .with_context(|| format!("{} is not gzip", path.display()))?;
            out
        } else {
            bytes
        };
        let mut snapshot: Self = serde_json::from_slice(&json)
            .with_context(|| format!("{} is not a snapshot", path.display()))?;
        snapshot.dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        Ok(snapshot)
    }

    /// Writes the catalogue file, gzipped when the file name ends in `.gz`.
    /// The pictures are files of their own, see [`Snapshot::fetch_pictures`].
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
            dir: PathBuf::new(),
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
        let expected = Snapshot {
            dir: dir.clone(),
            ..snapshot.clone()
        };
        for name in ["catalog.json", "catalog.json.gz"] {
            let path = dir.join(name);
            snapshot.write(&path).unwrap();
            assert_eq!(Snapshot::read(&path).unwrap(), expected, "{name}");
        }
        // The directory itself names the snapshot.
        assert_eq!(Snapshot::read(&dir).unwrap(), expected, "directory");
        assert!(
            std::fs::read(dir.join("catalog.json.gz"))
                .unwrap()
                .starts_with(&[0x1f, 0x8b]),
            "gzip magic"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_picture_without_a_file_is_fetched_from_its_address() {
        let seed: ImageSeed =
            serde_json::from_str(r#"{"url":"https://x/a.jpg","alt":"A"}"#).unwrap();
        assert_eq!(seed.file, None);
        let json = serde_json::to_string(&seed).unwrap();
        assert!(!json.contains("file"), "{json}");
    }

    #[test]
    fn a_category_is_in_the_menu_unless_said_otherwise() {
        let seed: CategorySeed = serde_json::from_str(r#"{"slug":"a","name":"A"}"#).unwrap();
        assert!(seed.in_menu);
    }
}
