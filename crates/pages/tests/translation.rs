mod common;

use common::*;
use evento::ProjectionAggregate;
use timada_pages::{CreateArticle, PageError, TextKind, TextTranslation};

const PAGE: TextKind = TextKind::Page;
const ARTICLE: TextKind = TextKind::Article;

fn titled(title: &str) -> TextTranslation {
    TextTranslation {
        title: title.to_owned(),
        ..Default::default()
    }
}

fn at(title: &str, slug: &str) -> TextTranslation {
    TextTranslation {
        slug: Some(slug.to_owned()),
        ..titled(title)
    }
}

#[tokio::test]
async fn a_page_is_worded_in_another_language_and_has_no_slug_there() {
    let pages = command().await;
    let terms = pages
        .create_page(ALICE, draft("Conditions de vente"))
        .await
        .unwrap();
    assert_eq!(
        pages
            .load_text_translation(PAGE, &terms, "en")
            .await
            .unwrap(),
        None
    );
    pages
        .translate_text(
            ALICE,
            PAGE,
            &terms,
            " EN ",
            TextTranslation {
                body: "## Article 1\r\n\r\nSales are final.\r\n".to_owned(),
                meta_title: "Terms".to_owned(),
                // A page has no summary, and the storefront decides where
                // it is.
                summary: "ignored".to_owned(),
                ..at("  Terms of sale ", "ignored")
            },
        )
        .await
        .unwrap();
    let english = pages
        .load_text_translation(PAGE, &terms, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.kind, "page");
    assert_eq!(english.subject_id, terms);
    assert_eq!(english.language, "en");
    assert_eq!(english.title, "Terms of sale");
    assert_eq!(english.slug, "");
    assert_eq!(english.summary, "");
    assert_eq!(english.body, "## Article 1\n\nSales are final.");
    assert_eq!(english.meta_title, "Terms");
    assert_eq!(english.aggregate_version().unwrap(), 2);

    // The page itself is untouched.
    let page = pages.load_page(&terms).await.unwrap().unwrap();
    assert_eq!(page.title, "Conditions de vente");
    assert_eq!(page.aggregate_version().unwrap(), 1);

    // The same words write nothing; new search engines' words write alone.
    let again = TextTranslation {
        body: "## Article 1\n\nSales are final.".to_owned(),
        meta_title: "Terms".to_owned(),
        ..titled("Terms of sale")
    };
    pages
        .translate_text(ALICE, PAGE, &terms, "en", again.clone())
        .await
        .unwrap();
    pages
        .translate_text(
            ALICE,
            PAGE,
            &terms,
            "en",
            TextTranslation {
                meta_title: "Terms of sale".to_owned(),
                ..again
            },
        )
        .await
        .unwrap();
    let english = pages
        .load_text_translation(PAGE, &terms, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.meta_title, "Terms of sale");
    assert_eq!(english.aggregate_version().unwrap(), 3);

    pages
        .translate_text(ALICE, PAGE, &terms, "de", titled("AGB"))
        .await
        .unwrap();
    let both = pages
        .load_text_translations(
            PAGE,
            &terms,
            &["en".to_owned(), "es".to_owned(), "de".to_owned()],
        )
        .await
        .unwrap();
    let titles: Vec<&str> = both.iter().map(|t| t.title.as_str()).collect();
    assert_eq!(titles, ["Terms of sale", "AGB"]);
}

#[tokio::test]
async fn an_article_holds_its_slug_once_in_a_language() {
    let pages = command().await;
    let guide = pages
        .create_article(ALICE, article_draft("Choisir sa taille"))
        .await
        .unwrap();
    let care = pages
        .create_article(ALICE, article_draft("Entretien"))
        .await
        .unwrap();
    pages
        .translate_text(
            ALICE,
            ARTICLE,
            &guide,
            "en",
            TextTranslation {
                summary: " How to choose,\nin two minutes. ".to_owned(),
                ..titled("Choosing a size")
            },
        )
        .await
        .unwrap();
    let english = pages
        .load_text_translation(ARTICLE, &guide, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.slug, "choosing-a-size");
    assert_eq!(english.summary, "How to choose, in two minutes.");

    // Another article cannot take it there, by its title or by asking;
    // elsewhere it is another slug.
    for text in [titled("Choosing a size"), at("Care", "choosing-a-size")] {
        let taken = pages
            .translate_text(ALICE, ARTICLE, &care, "en", text)
            .await;
        assert!(
            matches!(taken, Err(PageError::ArticleSlugTaken)),
            "{taken:?}"
        );
    }
    pages
        .translate_text(ALICE, ARTICLE, &care, "de", at("Pflege", "choosing-a-size"))
        .await
        .unwrap();
    // Another article's main slug is its address in every language it is
    // not translated into.
    let taken = pages
        .translate_text(ALICE, ARTICLE, &care, "en", at("Care", "choisir-sa-taille"))
        .await;
    assert!(
        matches!(taken, Err(PageError::ArticleSlugTaken)),
        "{taken:?}"
    );

    // A new title keeps the address; a new slug moves it, remembers the
    // old one and frees it at once.
    pages
        .translate_text(
            ALICE,
            ARTICLE,
            &guide,
            "en",
            titled("Choosing the right size"),
        )
        .await
        .unwrap();
    pages
        .translate_text(
            ALICE,
            ARTICLE,
            &guide,
            "en",
            at("Choosing the right size", "size-guide"),
        )
        .await
        .unwrap();
    let english = pages
        .load_text_translation(ARTICLE, &guide, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(english.slug, "size-guide");
    let former: Vec<&str> = english
        .former_slugs
        .iter()
        .map(|was| was.slug.as_str())
        .collect();
    assert_eq!(former, ["choosing-a-size"]);
    pages
        .translate_text(ALICE, ARTICLE, &care, "en", at("Care", "choosing-a-size"))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_dropped_translation_frees_its_slug_and_starts_again_empty() {
    let pages = command().await;
    let guide = pages
        .create_article(ALICE, article_draft("Choisir sa taille"))
        .await
        .unwrap();
    let care = pages
        .create_article(ALICE, article_draft("Entretien"))
        .await
        .unwrap();
    pages
        .translate_text(
            ALICE,
            ARTICLE,
            &guide,
            "en",
            TextTranslation {
                body: "By size.".to_owned(),
                meta_title: "Sizes".to_owned(),
                ..titled("Size guide")
            },
        )
        .await
        .unwrap();
    pages
        .drop_text_translation(ALICE, ARTICLE, &guide, "en")
        .await
        .unwrap();
    // Told twice, nothing more happens.
    pages
        .drop_text_translation(ALICE, ARTICLE, &guide, "en")
        .await
        .unwrap();
    assert_eq!(
        pages
            .load_text_translation(ARTICLE, &guide, "en")
            .await
            .unwrap(),
        None
    );
    let gone = pages
        .load_text_translation_any(ARTICLE, &guide, "en")
        .await
        .unwrap()
        .unwrap();
    assert!(gone.dropped);
    assert_eq!(gone.former_slugs.len(), 1);

    pages
        .translate_text(ALICE, ARTICLE, &care, "en", at("Care", "size-guide"))
        .await
        .unwrap();
    let taken = pages
        .translate_text(ALICE, ARTICLE, &guide, "en", titled("Size guide"))
        .await;
    assert!(
        matches!(taken, Err(PageError::ArticleSlugTaken)),
        "{taken:?}"
    );
    pages
        .translate_text(ALICE, ARTICLE, &guide, "en", at("Size guide", "sizes"))
        .await
        .unwrap();
    let back = pages
        .load_text_translation(ARTICLE, &guide, "en")
        .await
        .unwrap()
        .unwrap();
    assert_eq!((back.body.as_str(), back.meta_title.as_str()), ("", ""));

    // A deleted article's slug is free too.
    pages.delete_article(ALICE, &care).await.unwrap();
    pages
        .translate_text(ALICE, ARTICLE, &guide, "en", at("Size guide", "size-guide"))
        .await
        .unwrap();
}

#[tokio::test]
async fn what_cannot_be_worded_is_refused() {
    let pages = command().await;
    let terms = pages
        .create_page(ALICE, draft("Conditions de vente"))
        .await
        .unwrap();
    let guide = pages
        .create_article(ALICE, article_draft("Choisir sa taille"))
        .await
        .unwrap();

    let off_list = pages
        .translate_text(ALICE, PAGE, &terms, "zz", titled("X"))
        .await;
    assert!(
        matches!(off_list, Err(PageError::InvalidLanguage)),
        "{off_list:?}"
    );
    let untitled = pages
        .translate_text(ALICE, PAGE, &terms, "en", titled(" "))
        .await;
    assert!(
        matches!(untitled, Err(PageError::InvalidTitle)),
        "{untitled:?}"
    );
    let bad_slug = pages
        .translate_text(ALICE, ARTICLE, &guide, "en", at("Sizes", "Size Guide!"))
        .await;
    assert!(
        matches!(bad_slug, Err(PageError::InvalidSlug)),
        "{bad_slug:?}"
    );
    let long = pages
        .translate_text(
            ALICE,
            ARTICLE,
            &guide,
            "en",
            TextTranslation {
                summary: "x".repeat(301),
                ..titled("Sizes")
            },
        )
        .await;
    assert!(matches!(long, Err(PageError::InvalidSummary)), "{long:?}");
    // A page is no article, and the other way round.
    let missing = pages
        .translate_text(ALICE, PAGE, &guide, "en", titled("X"))
        .await;
    assert!(matches!(missing, Err(PageError::NotFound)), "{missing:?}");
    let no_article = pages
        .translate_text(ALICE, ARTICLE, &terms, "en", titled("X"))
        .await;
    assert!(
        matches!(no_article, Err(PageError::ArticleNotFound)),
        "{no_article:?}"
    );
    pages.delete_page(ALICE, &terms).await.unwrap();
    let deleted = pages
        .translate_text(ALICE, PAGE, &terms, "en", titled("Terms"))
        .await;
    assert!(matches!(deleted, Err(PageError::NotFound)), "{deleted:?}");
}

/// Under `/en` a translation's slug is its article's address: a main slug
/// landing on it would leave the other article without one there (#649).
#[tokio::test]
async fn a_main_slug_never_lands_on_another_articles_translation() {
    let pages = command().await;
    let guide = pages
        .create_article(ALICE, article_draft("Choisir sa taille"))
        .await
        .unwrap();
    let care = pages
        .create_article(ALICE, article_draft("Entretien"))
        .await
        .unwrap();
    pages
        .translate_text(ALICE, ARTICLE, &guide, "en", at("Choosing a size", "sizes"))
        .await
        .unwrap();

    // At a creation and at a rename alike.
    let taken = pages
        .create_article(
            ALICE,
            CreateArticle {
                slug: "sizes".to_owned(),
                ..article_draft("Sizes")
            },
        )
        .await;
    assert!(
        matches!(taken, Err(PageError::ArticleSlugTaken)),
        "{taken:?}"
    );
    let taken = pages.rename_article(ALICE, &care, "sizes").await;
    assert!(
        matches!(taken, Err(PageError::ArticleSlugTaken)),
        "{taken:?}"
    );
    // The refusal left nothing behind: the article keeps its address.
    let kept = pages.load_article(&care).await.unwrap().unwrap();
    assert_eq!(kept.slug, "entretien");

    // The article itself may go by the slug its own translation holds.
    pages.rename_article(ALICE, &guide, "sizes").await.unwrap();
    pages
        .rename_article(ALICE, &guide, "choisir-sa-taille")
        .await
        .unwrap();

    // A translation that moved to another slug, or was dropped, no longer
    // holds anything.
    pages
        .translate_text(
            ALICE,
            ARTICLE,
            &guide,
            "en",
            at("Choosing a size", "size-guide"),
        )
        .await
        .unwrap();
    pages.rename_article(ALICE, &care, "sizes").await.unwrap();
    pages
        .drop_text_translation(ALICE, ARTICLE, &guide, "en")
        .await
        .unwrap();
    pages
        .create_article(
            ALICE,
            CreateArticle {
                slug: "size-guide".to_owned(),
                ..article_draft("Size guide")
            },
        )
        .await
        .unwrap();
}
