//! A picture as the store records it: the two sizes it is kept in, never the
//! upload itself.
//!
//! A merchant uploads whatever the camera made, a few megabytes of JPEG;
//! a listing shows it a few hundred pixels wide. Keeping the upload on every
//! node and serving it as a thumbnail would cost each node the uploads of
//! every shop and each page view megabytes of pictures. So whoever takes an
//! upload renders it into two WebP files, one for listings and one for the
//! product page ([`Rendition`]), bounded in pixels and so in bytes, and the
//! store records those two files: their dimensions, their size and the
//! digest of their bytes ([`RenditionFile`]), which a node checks a copy
//! against before keeping it. The upload's own dimensions are recorded too
//! ([`Picture::width`], [`Picture::height`]), as what the merchant gave; its
//! bytes are not kept.
//!
//! The files live with the host, under the tenant and the rendition's
//! directory: `<tenant>/400/<id>.webp` and `<tenant>/1200/<id>.webp`
//! ([`Picture::file`]).

/// The two sizes a picture is kept in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rendition {
    /// For listings, cards, menus and thumbnails: 400 px on its longest
    /// side.
    Listing,
    /// For the product page and anywhere a shopper looks at the picture
    /// itself: 1200 px on its longest side.
    Page,
}

impl Rendition {
    pub const ALL: [Self; 2] = [Self::Listing, Self::Page];

    /// The most pixels the rendition has on its longest side. A smaller
    /// upload is kept at its own size, never enlarged.
    pub fn max_side(self) -> u32 {
        match self {
            Self::Listing => 400,
            Self::Page => 1200,
        }
    }

    /// The directory the rendition's files sit in, under the tenant's: its
    /// bound in pixels, so a browser's `srcset` reads naturally.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Listing => "400",
            Self::Page => "1200",
        }
    }

    /// The rendition a directory name stands for.
    pub fn from_dir(dir: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|rendition| rendition.dir() == dir)
    }
}

/// The format every rendition is kept in.
pub const RENDITION_EXT: &str = "webp";
pub const RENDITION_MIME: &str = "image/webp";

/// The most pixels an upload may have: a 25-megapixel photograph is as much
/// as a phone makes, and decoding one takes a hundred megabytes.
pub const MAX_PICTURE_PIXELS: u64 = 25_000_000;

/// One rendition's file, as the store records it and a node checks a copy
/// against.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct RenditionFile {
    pub width: u32,
    pub height: u32,
    /// In bytes.
    pub size: u64,
    /// SHA-256 of the bytes.
    pub sha256: [u8; 32],
}

impl RenditionFile {
    /// The digest as lowercase hex, for tables and logs.
    pub fn sha256_hex(&self) -> String {
        self.sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Whether the record describes a file at all: a key with `store:write`
    /// can record what it likes, and a zero-byte or zero-pixel file is not
    /// a picture.
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.size > 0
    }
}

/// A picture as the store records it: the upload's dimensions and its two
/// renditions. The bytes live with the host, named after the picture's id.
#[derive(
    Debug,
    Clone,
    Copy,
    Default,
    PartialEq,
    Eq,
    Hash,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Picture {
    /// Of the upload, as the merchant gave it.
    pub width: u32,
    pub height: u32,
    pub listing: RenditionFile,
    pub page: RenditionFile,
}

impl Picture {
    pub fn rendition(&self, rendition: Rendition) -> &RenditionFile {
        match rendition {
            Rendition::Listing => &self.listing,
            Rendition::Page => &self.page,
        }
    }

    /// The renditions with their files.
    pub fn files(&self) -> [(Rendition, &RenditionFile); 2] {
        [
            (Rendition::Listing, &self.listing),
            (Rendition::Page, &self.page),
        ]
    }

    /// What the picture's files take on disk, together.
    pub fn bytes(&self) -> u64 {
        self.listing.size + self.page.size
    }

    /// Whether every part describes a file; see [`RenditionFile::is_valid`].
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.listing.is_valid() && self.page.is_valid()
    }

    /// The path of one rendition's file below the tenant's directory, and
    /// below the tenant's address: `400/<id>.webp`.
    pub fn file(image_id: &str, rendition: Rendition) -> String {
        format!("{}/{image_id}.{RENDITION_EXT}", rendition.dir())
    }

    /// The address of one rendition's file, under the tenant's picture
    /// address (no trailing slash).
    pub fn url(base: &str, image_id: &str, rendition: Rendition) -> String {
        format!("{base}/{}", Self::file(image_id, rendition))
    }
}

/// Whether `name` is the file name a rendition is served under, `<id>.webp`
/// with a ULID for an id: what a page's body names a picture by.
pub fn is_picture_file(name: &str) -> bool {
    name.strip_suffix(".webp").is_some_and(|id| {
        id.len() == 26
            && id
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_files_by_rendition() {
        assert_eq!(Picture::file("01J", Rendition::Listing), "400/01J.webp");
        assert_eq!(
            Picture::url("https://m.example/t1", "01J", Rendition::Page),
            "https://m.example/t1/1200/01J.webp"
        );
        assert_eq!(Rendition::from_dir("400"), Some(Rendition::Listing));
        assert_eq!(Rendition::from_dir("1200"), Some(Rendition::Page));
        assert_eq!(Rendition::from_dir("800"), None);
        assert!(is_picture_file("01ARZ3NDEKTSV4RRFFQ69G5FAV.webp"));
        assert!(!is_picture_file("01ARZ3NDEKTSV4RRFFQ69G5FAV.png"));
        assert!(!is_picture_file("../x.webp"));
    }

    #[test]
    fn a_record_must_describe_a_file() {
        let file = RenditionFile {
            width: 1,
            height: 1,
            size: 1,
            sha256: [0xab; 32],
        };
        assert!(file.is_valid());
        assert_eq!(file.sha256_hex(), "ab".repeat(32));
        assert!(!RenditionFile::default().is_valid());
        let picture = Picture {
            width: 2,
            height: 2,
            listing: file,
            page: file,
        };
        assert!(picture.is_valid());
        assert_eq!(picture.bytes(), 2);
        assert!(
            !Picture {
                width: 0,
                ..picture
            }
            .is_valid()
        );
    }
}
