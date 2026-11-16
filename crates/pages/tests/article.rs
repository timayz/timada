mod common;

use common::*;
use timada_pages::{
    AddArticleImage, CreateArticle, PageError, articles_of_tenant, new_page_image_id,
    read_model_subscription,
};

use timada_core::page::PageFrom;

fn picture() -> timada_core::Picture {
    let rendition = |side: u32| timada_core::RenditionFile {
        width: side,
        height: side * 3 / 4,
        size: u64::from(side) * 100,
        sha256: [7; 32],
    };
    timada_core::Picture {
        width: 1600,
        height: 1200,
        listing: rendition(400),
        page: rendition(1200),
    }
}

#[tokio::test]
async fn an_article_starts_as_a_draft_under_a_slug_of_its_title() {
    let blog = command().await;
    let id = blog
        .create_article(ALICE, article_draft("Bien choisir son tapis"))
        .await
        .unwrap();
    let article = blog.load_article(&id).await.unwrap().unwrap();
    assert_eq!(article.slug, "bien-choisir-son-tapis");
    assert_eq!(article.summary, "Comment choisir, en deux minutes.");
    assert_eq!(article.body, "## Les tailles\n\nDu S au XL.");
    assert!(!article.published);
    assert_eq!(article.published_at, 0);
    let by_slug = blog
        .load_article_by_slug("bien-choisir-son-tapis")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(by_slug.id, id);

    let long = blog
        .create_article(
            ALICE,
            CreateArticle {
                summary: "x".repeat(301),
                ..article_draft("Long")
            },
        )
        .await;
    assert!(matches!(long, Err(PageError::InvalidSummary)), "{long:?}");
}

#[tokio::test]
async fn the_blogs_slugs_are_its_own() {
    let blog = command().await;
    // A page and an article may share a slug: they answer at two addresses.
    blog.create_page(ALICE, draft("Livraison")).await.unwrap();
    let id = blog
        .create_article(ALICE, article_draft("Livraison"))
        .await
        .unwrap();
    let again = blog.create_article(ALICE, article_draft("Livraison")).await;
    assert!(
        matches!(again, Err(PageError::ArticleSlugTaken)),
        "{again:?}"
    );

    // A rename frees the old slug, which the article remembers.
    blog.rename_article(ALICE, &id, "delais").await.unwrap();
    let renamed = blog.load_article(&id).await.unwrap().unwrap();
    assert_eq!(renamed.slug, "delais");
    assert_eq!(renamed.former_slugs[0].slug, "livraison");
    assert!(
        blog.load_article_by_slug("livraison")
            .await
            .unwrap()
            .is_none()
    );
    run_releases(&blog).await;
    assert!(
        blog.load_article_slug_index("livraison")
            .await
            .unwrap()
            .is_none()
    );
    // The page's entry was not the article's to free.
    assert!(
        blog.load_page_slug_index("livraison")
            .await
            .unwrap()
            .is_some()
    );
    blog.create_article(ALICE, article_draft("Livraison"))
        .await
        .unwrap();
}

#[tokio::test]
async fn publishing_dates_an_article_once() {
    let blog = command().await;
    let id = blog
        .create_article(ALICE, article_draft("Nouveautés"))
        .await
        .unwrap();
    blog.edit_article(ALICE, &id, " Les nouveautés ", "", "Texte.")
        .await
        .unwrap();
    blog.set_article_seo(ALICE, &id, "Nouveautés | Acme", "")
        .await
        .unwrap();
    blog.publish_article(ALICE, &id).await.unwrap();
    let article = blog.load_article(&id).await.unwrap().unwrap();
    assert!(article.published);
    assert_eq!(article.title, "Les nouveautés");
    assert_eq!(article.summary, "");
    assert_eq!(article.seo_title(), "Nouveautés | Acme");
    // Read back from the event, never reckoned from the test's clock.
    let first = article.published_at;
    assert_eq!(first, article.updated_at);

    // The merchant's date stands, through a draft and back.
    let day = first - 86_400 * 30;
    blog.date_article(ALICE, &id, day).await.unwrap();
    blog.unpublish_article(ALICE, &id).await.unwrap();
    blog.publish_article(ALICE, &id).await.unwrap();
    let article = blog.load_article(&id).await.unwrap().unwrap();
    assert!(article.published);
    assert_eq!(article.published_at, day);

    let ahead = blog.date_article(ALICE, &id, first + 86_400 * 2).await;
    assert!(
        matches!(ahead, Err(PageError::InvalidArticleDate)),
        "{ahead:?}"
    );
}

#[tokio::test]
async fn the_cover_is_one_of_the_articles_pictures() {
    let blog = command().await;
    let id = blog
        .create_article(ALICE, article_draft("Atelier"))
        .await
        .unwrap();
    let image_id = new_page_image_id();
    let image = AddArticleImage {
        image_id: image_id.clone(),
        picture: picture(),
    };
    blog.add_article_image(ALICE, &id, image.clone())
        .await
        .unwrap();
    // Twice is once.
    blog.add_article_image(ALICE, &id, image).await.unwrap();

    let other = blog.set_article_cover(ALICE, &id, Some("nope")).await;
    assert!(matches!(other, Err(PageError::ImageNotFound)), "{other:?}");
    blog.set_article_cover(ALICE, &id, Some(&image_id))
        .await
        .unwrap();
    let article = blog.load_article(&id).await.unwrap().unwrap();
    assert_eq!(article.images.len(), 1);
    assert_eq!(article.cover_image().unwrap().id, image_id);

    // The picture taken off takes the cover with it.
    blog.remove_article_image(ALICE, &id, &image_id)
        .await
        .unwrap();
    let article = blog.load_article(&id).await.unwrap().unwrap();
    assert!(article.images.is_empty());
    assert_eq!(article.cover, None);
}

#[tokio::test]
async fn a_deleted_article_frees_its_slug() {
    let blog = command().await;
    let id = blog
        .create_article(ALICE, article_draft("Soldes"))
        .await
        .unwrap();
    blog.publish_article(ALICE, &id).await.unwrap();
    let deleted = blog.delete_article(ALICE, &id).await.unwrap();
    assert_eq!(deleted.slug, "soldes");
    assert!(blog.load_article(&id).await.unwrap().is_none());
    let gone = blog.publish_article(ALICE, &id).await;
    assert!(matches!(gone, Err(PageError::ArticleNotFound)), "{gone:?}");
    blog.create_article(ALICE, article_draft("Soldes"))
        .await
        .unwrap();
}

#[tokio::test]
async fn the_list_reads_the_most_recent_first_page_after_page() {
    let (executor, db) = executor_with_pool().await;
    let blog = timada_pages::Command::new(executor.clone(), TENANT);
    let catch_up = async || {
        read_model_subscription::<evento::Turso>(db.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };
    let mut ids = Vec::new();
    for title in ["Un", "Deux", "Trois"] {
        let id = blog
            .create_article(ALICE, article_draft(title))
            .await
            .unwrap();
        blog.publish_article(ALICE, &id).await.unwrap();
        ids.push(id);
    }
    // Dated apart, whatever the clock said as they were written.
    let now = blog
        .load_article(&ids[2])
        .await
        .unwrap()
        .unwrap()
        .published_at;
    for (id, days) in ids.iter().zip([30, 20, 10]) {
        blog.date_article(ALICE, id, now - 86_400 * days)
            .await
            .unwrap();
    }
    catch_up().await;

    let first = articles_of_tenant(&db, TENANT, PageFrom::First, 2)
        .await
        .unwrap();
    let titles = |rows: &[timada_pages::ArticleRow]| {
        rows.iter().map(|row| row.title.clone()).collect::<Vec<_>>()
    };
    assert_eq!(titles(&first.rows), ["Trois", "Deux"]);
    assert!(first.rows[0].published);
    assert_eq!(first.newer, None);
    let older = first.older.expect("a second page");
    let second = articles_of_tenant(&db, TENANT, PageFrom::Older(&older), 2)
        .await
        .unwrap();
    assert_eq!(titles(&second.rows), ["Un"]);
    assert_eq!(second.older, None);

    blog.delete_article(ALICE, &ids[2]).await.unwrap();
    catch_up().await;
    let left = articles_of_tenant(&db, TENANT, PageFrom::First, 10)
        .await
        .unwrap();
    assert_eq!(titles(&left.rows), ["Deux", "Un"]);
    assert!(
        articles_of_tenant(&db, "other", PageFrom::First, 10)
            .await
            .unwrap()
            .rows
            .is_empty()
    );
}

/// A page of the list reads its rows off the index, in order.
#[tokio::test]
async fn a_page_of_the_list_walks_the_index() {
    let (_, db) = executor_with_pool().await;
    for older in [true, false] {
        let statement = if older {
            "SELECT id FROM articles INDEXED BY articles_listed
             WHERE tenant_id = ?1 AND listed_at <= ?2 AND (listed_at < ?2 OR id < ?3)
             ORDER BY listed_at DESC, id DESC LIMIT ?4"
        } else {
            "SELECT id FROM articles INDEXED BY articles_listed
             WHERE tenant_id = ?1 AND listed_at >= ?2 AND (listed_at > ?2 OR id > ?3)
             ORDER BY listed_at ASC, id ASC LIMIT ?4"
        };
        let plan: Vec<String> = db
            .fetch_all::<(i64, i64, i64, String)>(
                &format!("EXPLAIN QUERY PLAN {statement}"),
                (TENANT, 0, "", 10),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|(_, _, _, detail)| detail)
            .collect();
        assert!(
            plan.iter()
                .all(|step| step.starts_with("SEARCH") && !step.contains("TEMP B-TREE")),
            "{plan:#?}"
        );
    }
}
