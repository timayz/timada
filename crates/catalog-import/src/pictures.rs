//! The pictures of a snapshot, kept beside its catalogue so a seed needs
//! no network: each is fetched once from the shop into `pictures/`, named
//! after its address, and the catalogue points at the file.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};
use timada_catalog::MAX_IMAGE_BYTES;

use crate::snapshot::{ImageSeed, PICTURES_DIR, Snapshot};

/// What fetching a snapshot's pictures did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PicturesReport {
    /// Distinct addresses whose file was already there.
    pub kept: usize,
    /// Distinct addresses fetched and written.
    pub fetched: usize,
    /// Distinct addresses left to the seed to fetch, with why.
    pub failed: Vec<String>,
}

impl Snapshot {
    /// Fetches into `dir/pictures/` every picture the catalogue names and
    /// the directory lacks, `concurrency` at a time, and points each
    /// [`ImageSeed::file`] at its file; a fetch that fails leaves the
    /// address alone for the seed to try. Pictures the shop serves at one
    /// address are one file, so taking the snapshot again costs only what
    /// changed. Sets [`Snapshot::dir`].
    pub async fn fetch_pictures(
        &mut self,
        client: &reqwest::Client,
        dir: &Path,
        concurrency: usize,
    ) -> anyhow::Result<PicturesReport> {
        let pictures = dir.join(PICTURES_DIR);
        std::fs::create_dir_all(&pictures)
            .with_context(|| format!("cannot create {}", pictures.display()))?;
        // One entry per address, whatever repeats it.
        let mut files: BTreeMap<String, Option<String>> = BTreeMap::new();
        for image in self.images() {
            files.entry(image.url.clone()).or_insert(None);
        }
        let mut report = PicturesReport::default();
        let mut pending: Vec<(String, String)> = Vec::new();
        for (url, file) in files.iter_mut() {
            let name = picture_file(url);
            if pictures.join(&name).is_file() {
                *file = Some(format!("{PICTURES_DIR}/{name}"));
                report.kept += 1;
            } else {
                pending.push((url.clone(), name));
            }
        }
        tracing::info!(kept = report.kept, to_fetch = pending.len(), "pictures");
        let total = pending.len();
        let mut done = 0;
        for chunk in pending.chunks(concurrency.max(1)) {
            let mut set = tokio::task::JoinSet::new();
            for (url, name) in chunk {
                let client = client.clone();
                let url = url.clone();
                let path = pictures.join(name);
                let name = name.clone();
                set.spawn(async move {
                    let result = fetch_one(&client, &url, &path).await;
                    (url, name, result)
                });
            }
            while let Some(joined) = set.join_next().await {
                let (url, name, result) = joined?;
                match result {
                    Ok(()) => {
                        files.insert(url, Some(format!("{PICTURES_DIR}/{name}")));
                        report.fetched += 1;
                    }
                    Err(err) => {
                        tracing::warn!(%url, %err, "picture not fetched");
                        report.failed.push(format!("{url}: {err:#}"));
                    }
                }
            }
            done += chunk.len();
            if done % 500 < chunk.len() || done == total {
                tracing::info!(done, total, "pictures fetched");
            }
        }
        for image in self.images_mut() {
            image.file = files.get(&image.url).cloned().flatten();
        }
        self.dir = dir.to_path_buf();
        Ok(report)
    }

    /// Every picture the catalogue names: the categories' and the products'.
    pub fn images(&self) -> impl Iterator<Item = &ImageSeed> {
        self.categories
            .iter()
            .filter_map(|c| c.image.as_ref())
            .chain(self.products.iter().flat_map(|p| p.images.iter()))
    }

    fn images_mut(&mut self) -> impl Iterator<Item = &mut ImageSeed> {
        self.categories
            .iter_mut()
            .filter_map(|c| c.image.as_mut())
            .chain(self.products.iter_mut().flat_map(|p| p.images.iter_mut()))
    }

    /// Where a picture's bytes are, when the snapshot carries them.
    pub fn picture_path(&self, image: &ImageSeed) -> Option<PathBuf> {
        image.file.as_ref().map(|file| self.dir.join(file))
    }
}

/// The file an address is kept under: a digest of the address, so one
/// address is one file whatever product names it, with the address's own
/// extension so the file opens as what it is.
pub fn picture_file(url: &str) -> String {
    let digest = Sha256::digest(url.as_bytes());
    let mut name = String::with_capacity(70);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(name, "{byte:02x}");
    }
    let ext = url
        .split(['?', '#'])
        .next()
        .and_then(|path| path.rsplit('/').next())
        .and_then(|file| file.rsplit_once('.'))
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        })
        .unwrap_or_else(|| "img".to_owned());
    name.push('.');
    name.push_str(&ext);
    name
}

/// One picture to one file, written whole or not at all, each try twice.
async fn fetch_one(client: &reqwest::Client, url: &str, path: &Path) -> anyhow::Result<()> {
    let mut attempt = 0;
    let bytes = loop {
        attempt += 1;
        match download(client, url).await {
            Ok(bytes) => break bytes,
            Err(err) if attempt < 2 => {
                tracing::debug!(url, %err, "retrying");
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
            Err(err) => return Err(err),
        }
    };
    let tmp = path.with_extension("part");
    tokio::fs::write(&tmp, &bytes)
        .await
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    tokio::fs::rename(&tmp, path)
        .await
        .with_context(|| format!("cannot move {} into place", tmp.display()))?;
    Ok(())
}

async fn download(client: &reqwest::Client, url: &str) -> anyhow::Result<bytes::Bytes> {
    let response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_IMAGE_BYTES)
    {
        anyhow::bail!("larger than the catalog takes");
    }
    let bytes = response.bytes().await?;
    anyhow::ensure!(!bytes.is_empty(), "empty");
    anyhow::ensure!(
        bytes.len() as u64 <= MAX_IMAGE_BYTES,
        "larger than the catalog takes"
    );
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::{CategorySeed, ProductSeed};

    #[test]
    fn a_file_is_named_after_its_address() {
        let a = picture_file("https://shop.example/wp-content/uploads/2026/07/IMG_1.jpg");
        let b = picture_file("https://shop.example/wp-content/uploads/2025/01/IMG_1.jpg");
        assert_ne!(a, b, "same file name, different addresses");
        assert!(a.ends_with(".jpg") && a.len() == 64 + 4, "{a}");
        assert_eq!(
            picture_file("https://shop.example/a.PNG?w=300"),
            picture_file("https://shop.example/a.PNG?w=300")
        );
        assert!(picture_file("https://shop.example/a.PNG?w=300").ends_with(".png"));
        assert!(picture_file("https://shop.example/picture").ends_with(".img"));
        assert!(picture_file("https://shop.example/a.tar.gz.backup").ends_with(".img"));
    }

    #[tokio::test]
    async fn a_picture_already_there_is_kept_and_one_out_of_reach_left_to_the_seed() {
        let dir = std::env::temp_dir().join(format!("timada-pictures-{}", std::process::id()));
        let pictures = dir.join(PICTURES_DIR);
        std::fs::create_dir_all(&pictures).unwrap();
        let present = "https://shop.example/present.jpg";
        // Nothing listens on port 9: the fetch fails at once.
        let absent = "http://127.0.0.1:9/absent.jpg";
        std::fs::write(pictures.join(picture_file(present)), b"jpeg bytes").unwrap();
        let mut snapshot = Snapshot {
            categories: vec![CategorySeed {
                slug: "c".into(),
                name: "C".into(),
                image: Some(ImageSeed {
                    url: present.into(),
                    alt: String::new(),
                    file: None,
                }),
                ..Default::default()
            }],
            products: vec![ProductSeed {
                slug: "p".into(),
                name: "P".into(),
                currency: "EUR".into(),
                amount_cents: Some(100),
                images: vec![
                    ImageSeed {
                        url: present.into(),
                        alt: String::new(),
                        file: None,
                    },
                    ImageSeed {
                        url: absent.into(),
                        alt: String::new(),
                        file: None,
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let report = snapshot.fetch_pictures(&client, &dir, 4).await.unwrap();
        assert_eq!(report.kept, 1, "the address named twice is one file");
        assert_eq!(report.fetched, 0);
        assert_eq!(report.failed.len(), 1, "{:?}", report.failed);
        assert!(report.failed[0].starts_with(absent));

        let expected = format!("{PICTURES_DIR}/{}", picture_file(present));
        assert_eq!(
            snapshot.categories[0]
                .image
                .as_ref()
                .unwrap()
                .file
                .as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            snapshot.products[0].images[0].file.as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(snapshot.products[0].images[1].file, None);
        assert_eq!(snapshot.dir, dir);
        assert_eq!(
            std::fs::read(
                snapshot
                    .picture_path(&snapshot.products[0].images[0])
                    .unwrap()
            )
            .unwrap(),
            b"jpeg bytes"
        );
        assert_eq!(snapshot.picture_path(&snapshot.products[0].images[1]), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
