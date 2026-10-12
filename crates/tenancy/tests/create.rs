mod common;

use common::*;
use timada_tenancy::{CreateTenant, MAX_SLUG_LEN, ROLE_OWNER, SLUG_SUFFIX_LEN, TenancyError};

#[tokio::test]
async fn creating_a_tenant_makes_its_creator_the_owner() {
    let cmd = command().await;
    let alice = new_id();
    let tenant = cmd
        .create_tenant(CreateTenant {
            name: "  Acme Shop  ".into(),
            owner_user_id: alice.clone(),
        })
        .await
        .unwrap();
    assert_eq!(tenant.slug, "acme-shop", "the slug derives from the name");
    assert_eq!(tenant.name, "Acme Shop");
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
async fn a_taken_slug_gets_a_short_suffix() {
    let cmd = command().await;
    let first = create_tenant(&cmd, "Acme", &new_id()).await;
    assert_eq!(first.slug, "acme");
    let second = create_tenant(&cmd, "ACME!", &new_id()).await;
    assert!(second.slug.starts_with("acme-"), "{}", second.slug);
    assert_eq!(second.slug.len(), "acme-".len() + SLUG_SUFFIX_LEN);
    let third = create_tenant(&cmd, "acme", &new_id()).await;
    assert_ne!(third.slug, second.slug);
    assert_eq!(
        cmd.load_tenant_by_slug("acme").await.unwrap().unwrap().id,
        first.id,
        "the first tenant keeps the plain slug"
    );
    assert_eq!(
        cmd.load_tenant_by_slug(&second.slug)
            .await
            .unwrap()
            .unwrap()
            .id,
        second.id
    );

    // A name at the slug limit still gets a suffix that fits.
    let long = "b".repeat(MAX_SLUG_LEN);
    let a = create_tenant(&cmd, &long, &new_id()).await;
    assert_eq!(a.slug.len(), MAX_SLUG_LEN);
    let b = create_tenant(&cmd, &long, &new_id()).await;
    assert_eq!(b.slug.len(), MAX_SLUG_LEN, "{}", b.slug);
    assert_ne!(a.slug, b.slug);
}

#[tokio::test]
async fn unusable_names_fall_back_to_a_default_slug() {
    let cmd = command().await;
    let tenant = create_tenant(&cmd, "!!!", &new_id()).await;
    assert_eq!(tenant.slug, "tenant");
    assert_eq!(tenant.name, "!!!");
}

#[tokio::test]
async fn bad_names_are_refused_before_anything_is_written() {
    let cmd = command().await;
    for name in ["", "   ", &"x".repeat(81)] {
        let err = cmd.create_tenant(create(name, "u")).await.unwrap_err();
        assert!(matches!(err, TenancyError::InvalidName), "{name:?}: {err}");
    }
    assert!(
        cmd.load_slug_index("tenant").await.unwrap().is_none(),
        "no slug was claimed by the refused creations"
    );
}

#[tokio::test]
async fn unknown_slugs_resolve_to_nothing() {
    let cmd = command().await;
    assert_eq!(cmd.load_tenant_by_slug("nobody").await.unwrap(), None);
}
