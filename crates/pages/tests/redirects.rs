mod common;

use common::*;
use timada_pages::{MAX_REDIRECT_PATH_LEN, PageError};

#[tokio::test]
async fn an_old_path_is_sent_to_a_path_of_the_shop() {
    let pages = command().await;
    assert!(pages.redirects().await.unwrap().is_empty());
    let id = pages
        .add_redirect(ALICE, " /products/blue-mug/ ", " /p/blue-mug?v=01 ")
        .await
        .unwrap();
    let all = pages.redirects().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].id, id);
    assert_eq!(all[0].from, "/products/blue-mug", "no trailing slash");
    assert_eq!(all[0].to, "/p/blue-mug?v=01");

    pages.remove_redirect(ALICE, &id).await.unwrap();
    assert!(pages.redirects().await.unwrap().is_empty());
    let gone = pages.remove_redirect(ALICE, &id).await;
    assert!(matches!(gone, Err(PageError::RedirectNotFound)), "{gone:?}");
}

#[tokio::test]
async fn a_redirect_is_checked() {
    let pages = command().await;
    let long = format!("/{}", "a".repeat(MAX_REDIRECT_PATH_LEN));
    for (from, to) in [
        ("products/mug", "/p/mug"),
        ("/", "/p/mug"),
        ("//evil.example/x", "/p/mug"),
        ("/products/mug?id=1", "/p/mug"),
        ("/products/mug#top", "/p/mug"),
        ("/old path", "/p/mug"),
        ("/café", "/p/mug"),
        ("/products/mug", "https://elsewhere.example/"),
        ("/products/mug", "//elsewhere.example/"),
        ("/products/mug", "p/mug"),
        ("/products/mug", ""),
        (long.as_str(), "/p/mug"),
    ] {
        let refused = pages.add_redirect(ALICE, from, to).await;
        assert!(
            matches!(refused, Err(PageError::InvalidRedirect)),
            "{from} → {to}: {refused:?}"
        );
    }
    assert!(pages.redirects().await.unwrap().is_empty());
}

#[tokio::test]
async fn an_old_path_redirects_once_and_never_to_another_redirect() {
    let pages = command().await;
    pages
        .add_redirect(ALICE, "/products/mug", "/p/mug")
        .await
        .unwrap();
    let twice = pages.add_redirect(ALICE, "/products/mug/", "/p/cup").await;
    assert!(matches!(twice, Err(PageError::RedirectExists)), "{twice:?}");
    for (from, to) in [
        // To itself, to an old path, and from where another one sends.
        ("/old", "/old?x=1"),
        ("/older", "/products/mug"),
        ("/p/mug", "/p/cup"),
    ] {
        let chain = pages.add_redirect(ALICE, from, to).await;
        assert!(
            matches!(chain, Err(PageError::RedirectChain)),
            "{from} → {to}: {chain:?}"
        );
    }
    // Two old paths may send to the same place.
    pages
        .add_redirect(ALICE, "/produits/mug", "/p/mug")
        .await
        .unwrap();
    assert_eq!(pages.redirects().await.unwrap().len(), 2);
}
