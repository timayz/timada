mod common;

use common::*;
use timada_tenancy::{CreateTenant, ROLE_OWNER, TenancyError};

#[tokio::test]
async fn creating_a_tenant_makes_its_creator_the_owner() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = cmd
        .create_tenant(CreateTenant {
            slug: " Acme-Shop ".into(),
            name: "  Acme  ".into(),
            owner_user_id: alice.clone(),
        })
        .await
        .unwrap();
    assert_eq!(tenant.slug, "acme-shop");
    assert_eq!(tenant.name, "Acme");
    assert_eq!(tenant.created_by, alice);
    assert!(!tenant.is_suspended());
    assert!(tenant.is_owner(&alice));
    assert_eq!(tenant.role_of(&alice), Some(ROLE_OWNER));
    assert_eq!(tenant.members.len(), 1);

    let by_slug = cmd.load_tenant_by_slug("acme-shop").await.unwrap().unwrap();
    assert_eq!(by_slug, tenant);
    assert_eq!(cmd.load_tenant(&tenant.id).await.unwrap(), Some(tenant));
}

#[tokio::test]
async fn slugs_are_unique() {
    let cmd = command().await;
    let first = create_tenant(&cmd, "acme", &new_id()).await;
    let err = cmd
        .create_tenant(create("ACME", &new_id()))
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::SlugTaken), "{err}");
    assert!(err.is_user_facing());
    assert_eq!(
        cmd.load_tenant_by_slug("acme").await.unwrap().unwrap().id,
        first.id,
        "the first tenant keeps the slug"
    );
}

#[tokio::test]
async fn bad_slugs_and_names_are_refused_before_anything_is_written() {
    let cmd = command().await;
    for slug in ["", "a", "-acme", "ac me", &"x".repeat(33)] {
        let err = cmd.create_tenant(create(slug, "u")).await.unwrap_err();
        assert!(matches!(err, TenancyError::InvalidSlug), "{slug:?}: {err}");
    }
    let err = cmd
        .create_tenant(CreateTenant {
            slug: "acme".into(),
            name: "   ".into(),
            owner_user_id: "u".into(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, TenancyError::InvalidName), "{err}");
    assert!(
        cmd.load_slug_index("acme").await.unwrap().is_none(),
        "the slug was not claimed by the refused creation"
    );
}

#[tokio::test]
async fn unknown_slugs_resolve_to_nothing() {
    let cmd = command().await;
    assert_eq!(cmd.load_tenant_by_slug("nobody").await.unwrap(), None);
}
