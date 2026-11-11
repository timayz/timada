mod common;

use common::*;
use timada_catalog::{AddImage, CatalogError, MAX_ALT_LEN, MAX_IMAGES_PER_PRODUCT};
use timada_core::{Picture, Rendition, RenditionFile};

fn rendition(side: u32, size: u64) -> RenditionFile {
    RenditionFile {
        width: side,
        height: side * 3 / 4,
        size,
        sha256: [side as u8; 32],
    }
}

fn picture() -> Picture {
    Picture {
        width: 3000,
        height: 2250,
        listing: rendition(400, 20_480),
        page: rendition(1200, 122_880),
    }
}

fn image(id: &str) -> AddImage {
    AddImage {
        image_id: id.to_owned(),
        picture: picture(),
        alt: "  A blue mug  ".into(),
    }
}

#[tokio::test]
async fn a_picture_joins_the_gallery_with_what_is_known_about_it() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    cmd.add_image(ALICE, &product.id, image("img-1"))
        .await
        .unwrap();
    let with_image = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(with_image.images.len(), 1);
    let first = &with_image.images[0];
    assert_eq!(first.id, "img-1");
    assert_eq!(first.picture, picture());
    assert_eq!((first.picture.width, first.picture.height), (3000, 2250));
    assert_eq!(first.picture.listing.width, 400);
    assert_eq!(first.picture.page.size, 122_880);
    assert_eq!(first.alt, "A blue mug");
    assert_eq!(
        Picture::file(&first.id, Rendition::Listing),
        "400/img-1.webp"
    );

    cmd.change_image_alt(ALICE, &product.id, "img-1", "A mug, in blue")
        .await
        .unwrap();
    let changed = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(changed.images[0].alt, "A mug, in blue");
    let version = changed.aggregate_version;
    cmd.change_image_alt(ALICE, &product.id, "img-1", " A mug, in blue ")
        .await
        .unwrap();
    assert_eq!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version,
        "the same text writes nothing"
    );
}

#[tokio::test]
async fn only_a_picture_of_a_known_kind_and_size_is_taken() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;

    // A record that describes no file: a zero-byte or zero-pixel rendition,
    // or no upload size.
    let empty = RenditionFile::default();
    for (what, bad) in [
        (
            "no upload size",
            Picture {
                width: 0,
                ..picture()
            },
        ),
        (
            "an empty listing",
            Picture {
                listing: empty,
                ..picture()
            },
        ),
        (
            "an empty page",
            Picture {
                page: RenditionFile {
                    size: 0,
                    ..rendition(1200, 1)
                },
                ..picture()
            },
        ),
    ] {
        let err = cmd
            .add_image(
                ALICE,
                &product.id,
                AddImage {
                    picture: bad,
                    ..image("img-bad")
                },
            )
            .await;
        assert!(
            matches!(err, Err(CatalogError::InvalidImage)),
            "{what}: {err:?}"
        );
    }
    let no_id = cmd
        .add_image(
            ALICE,
            &product.id,
            AddImage {
                image_id: "  ".into(),
                ..image("img-bad")
            },
        )
        .await;
    assert!(
        matches!(no_id, Err(CatalogError::InvalidImage)),
        "{no_id:?}"
    );
    let long_alt = cmd
        .add_image(
            ALICE,
            &product.id,
            AddImage {
                alt: "a".repeat(MAX_ALT_LEN + 1),
                ..image("img-bad")
            },
        )
        .await;
    assert!(
        matches!(long_alt, Err(CatalogError::InvalidAlt)),
        "{long_alt:?}"
    );

    cmd.add_image(ALICE, &product.id, image("img-1"))
        .await
        .unwrap();
    let twice = cmd.add_image(ALICE, &product.id, image("img-1")).await;
    assert!(
        matches!(twice, Err(CatalogError::InvalidImage)),
        "the same id twice: {twice:?}"
    );
    let missing = cmd
        .change_image_alt(ALICE, &product.id, "img-nope", "Nothing")
        .await;
    assert!(
        matches!(missing, Err(CatalogError::ImageNotFound)),
        "{missing:?}"
    );
}

#[tokio::test]
async fn a_gallery_is_reordered_as_a_whole() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    for n in 1..=3 {
        cmd.add_image(ALICE, &product.id, image(&format!("img-{n}")))
            .await
            .unwrap();
    }
    let order = |p: &timada_catalog::ProductState| -> Vec<String> {
        p.images.iter().map(|i| i.id.clone()).collect()
    };
    assert_eq!(
        order(&cmd.load_product(&product.id).await.unwrap().unwrap()),
        ["img-1", "img-2", "img-3"],
        "a new picture goes last"
    );

    cmd.reorder_images(
        ALICE,
        &product.id,
        vec!["img-3".into(), "img-1".into(), "img-2".into()],
    )
    .await
    .unwrap();
    let reordered = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(order(&reordered), ["img-3", "img-1", "img-2"]);

    let version = reordered.aggregate_version;
    cmd.reorder_images(
        ALICE,
        &product.id,
        vec!["img-3".into(), "img-1".into(), "img-2".into()],
    )
    .await
    .unwrap();
    assert_eq!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .aggregate_version,
        version,
        "the same order writes nothing"
    );

    for bad in [
        vec!["img-1".into()],
        vec!["img-1".into(), "img-1".into(), "img-2".into()],
        vec!["img-1".into(), "img-2".into(), "img-nope".into()],
    ] {
        let err = cmd.reorder_images(ALICE, &product.id, bad.clone()).await;
        assert!(
            matches!(err, Err(CatalogError::InvalidImageOrder)),
            "{bad:?}: {err:?}"
        );
    }
}

#[tokio::test]
async fn a_removed_picture_leaves_the_gallery_and_makes_room() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    for n in 0..MAX_IMAGES_PER_PRODUCT {
        cmd.add_image(ALICE, &product.id, image(&format!("img-{n}")))
            .await
            .unwrap();
    }
    let full = cmd.add_image(ALICE, &product.id, image("one-more")).await;
    assert!(matches!(full, Err(CatalogError::TooManyImages)), "{full:?}");

    cmd.remove_image(ALICE, &product.id, "img-0").await.unwrap();
    let smaller = cmd.load_product(&product.id).await.unwrap().unwrap();
    assert_eq!(smaller.images.len(), MAX_IMAGES_PER_PRODUCT - 1);
    assert!(!smaller.images.iter().any(|i| i.id == "img-0"));
    cmd.remove_image(ALICE, &product.id, "img-0")
        .await
        .expect("removing twice is a no-op");
    cmd.add_image(ALICE, &product.id, image("one-more"))
        .await
        .expect("and there is room again");
}

#[tokio::test]
async fn an_archived_product_keeps_its_gallery_as_it_was() {
    let cmd = command().await;
    let product = create_product(&cmd, "Blue Mug").await;
    cmd.add_image(ALICE, &product.id, image("img-1"))
        .await
        .unwrap();
    cmd.archive_product(ALICE, &product.id).await.unwrap();
    for err in [
        cmd.add_image(ALICE, &product.id, image("img-2")).await,
        cmd.change_image_alt(ALICE, &product.id, "img-1", "Nope")
            .await,
        cmd.reorder_images(ALICE, &product.id, vec!["img-1".into()])
            .await,
        cmd.remove_image(ALICE, &product.id, "img-1").await,
    ] {
        assert!(matches!(err, Err(CatalogError::Archived)), "{err:?}");
    }
    assert_eq!(
        cmd.load_product(&product.id)
            .await
            .unwrap()
            .unwrap()
            .images
            .len(),
        1
    );
}
