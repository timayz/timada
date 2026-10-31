mod common;

use common::*;
use timada_pages::{AddPageImage, CreatePage, PageError, new_page_image_id};

#[tokio::test]
async fn a_page_starts_as_a_draft_under_a_slug_of_its_title() {
    let pages = command().await;
    let id = pages
        .create_page(ALICE, draft("Conditions générales de vente"))
        .await
        .unwrap();
    let page = pages.load_page(&id).await.unwrap().unwrap();
    assert_eq!(page.slug, "conditions-generales-de-vente");
    assert_eq!(page.title, "Conditions générales de vente");
    assert_eq!(page.body, "## Article 1\n\nLes ventes sont fermes.");
    assert!(!page.published);
    let by_slug = pages
        .load_page_by_slug("conditions-generales-de-vente")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_slug.id, id);
}

#[tokio::test]
async fn a_slug_belongs_to_one_page() {
    let pages = command().await;
    pages
        .create_page(ALICE, draft("Mentions légales"))
        .await
        .unwrap();
    let again = pages.create_page(ALICE, draft("Mentions légales")).await;
    assert!(matches!(again, Err(PageError::SlugTaken)), "{again:?}");
    let invalid = pages
        .create_page(
            ALICE,
            CreatePage {
                slug: "Not a slug!".to_owned(),
                ..draft("About")
            },
        )
        .await;
    assert!(
        matches!(invalid, Err(PageError::InvalidSlug)),
        "{invalid:?}"
    );
}

#[tokio::test]
async fn renaming_frees_the_old_slug() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("About us")).await.unwrap();
    let other = pages.create_page(ALICE, draft("Contact")).await.unwrap();
    let taken = pages.rename_page(ALICE, &id, "contact").await;
    assert!(matches!(taken, Err(PageError::SlugTaken)), "{taken:?}");

    pages
        .rename_page(ALICE, &id, "qui-sommes-nous")
        .await
        .unwrap();
    assert_eq!(
        pages.load_page(&id).await.unwrap().unwrap().slug,
        "qui-sommes-nous"
    );
    assert!(pages.load_page_by_slug("about-us").await.unwrap().is_none());
    // The old slug is free for another page.
    pages.rename_page(ALICE, &other, "about-us").await.unwrap();
    assert_eq!(
        pages
            .load_page_by_slug("about-us")
            .await
            .unwrap()
            .unwrap()
            .id,
        other
    );
}

#[tokio::test]
async fn edit_seo_and_publish() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("Livraison")).await.unwrap();
    pages
        .edit_page(ALICE, &id, " Politique de livraison ", "Sous 48 h.")
        .await
        .unwrap();
    pages
        .set_page_seo(ALICE, &id, "Livraison | Acme", "  Délais  et\ntarifs. ")
        .await
        .unwrap();
    let too_long = pages.set_page_seo(ALICE, &id, &"x".repeat(71), "").await;
    assert!(
        matches!(too_long, Err(PageError::InvalidSeo)),
        "{too_long:?}"
    );
    let page = pages.load_page(&id).await.unwrap().unwrap();
    assert_eq!(page.title, "Politique de livraison");
    assert_eq!(page.body, "Sous 48 h.");
    assert_eq!(page.seo_title(), "Livraison | Acme");
    assert_eq!(page.meta_description, "Délais et tarifs.");
    assert!(!page.published);

    pages.publish_page(ALICE, &id).await.unwrap();
    assert!(pages.load_page(&id).await.unwrap().unwrap().published);
    pages.unpublish_page(ALICE, &id).await.unwrap();
    assert!(!pages.load_page(&id).await.unwrap().unwrap().published);
}

#[tokio::test]
async fn pictures_are_added_and_removed() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("Atelier")).await.unwrap();
    let image_id = new_page_image_id();
    let image = AddPageImage {
        image_id: image_id.clone(),
        ext: "webp".to_owned(),
        size: 1234,
        width: Some(800),
        height: Some(600),
    };
    pages
        .add_page_image(ALICE, &id, image.clone())
        .await
        .unwrap();
    // Twice is once.
    pages.add_page_image(ALICE, &id, image).await.unwrap();
    let page = pages.load_page(&id).await.unwrap().unwrap();
    assert_eq!(page.images.len(), 1);
    assert_eq!(page.images[0].mime, "image/webp");
    assert_eq!(page.images[0].file(), format!("{image_id}.webp"));
    let bmp = pages
        .add_page_image(
            ALICE,
            &id,
            AddPageImage {
                image_id: new_page_image_id(),
                ext: "bmp".to_owned(),
                ..AddPageImage::default()
            },
        )
        .await;
    assert!(matches!(bmp, Err(PageError::UnsupportedImage)), "{bmp:?}");

    pages
        .remove_page_image(ALICE, &id, &image_id)
        .await
        .unwrap();
    assert!(
        pages
            .load_page(&id)
            .await
            .unwrap()
            .unwrap()
            .images
            .is_empty()
    );
    let gone = pages.remove_page_image(ALICE, &id, &image_id).await;
    assert!(matches!(gone, Err(PageError::ImageNotFound)), "{gone:?}");
}

#[tokio::test]
async fn a_deleted_page_frees_its_slug() {
    let pages = command().await;
    let id = pages.create_page(ALICE, draft("Old terms")).await.unwrap();
    pages.publish_page(ALICE, &id).await.unwrap();
    let deleted = pages.delete_page(ALICE, &id).await.unwrap();
    assert_eq!(deleted.slug, "old-terms");
    assert!(pages.load_page(&id).await.unwrap().is_none());
    assert!(
        pages
            .load_page_by_slug("old-terms")
            .await
            .unwrap()
            .is_none()
    );
    let gone = pages.publish_page(ALICE, &id).await;
    assert!(matches!(gone, Err(PageError::NotFound)), "{gone:?}");
    // The slug is taken again by a new page.
    let new = pages.create_page(ALICE, draft("Old terms")).await.unwrap();
    assert_eq!(
        pages
            .load_page_by_slug("old-terms")
            .await
            .unwrap()
            .unwrap()
            .id,
        new
    );
}
