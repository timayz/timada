mod common;

use common::*;
use timada_tenancy::{
    Command, count_tenants, group_of, list_tenants, read_model_subscription, tenant_by_group,
    tenants_of_user,
};

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool) {
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(executor)
        .await
        .unwrap();
}

#[tokio::test]
async fn tenants_and_members_are_listed_and_stay_idempotent() {
    let (executor, pool) = executor_with_pool().await;
    let cmd = Command(executor.clone());
    let alice = new_id();
    let bob = new_id();
    let acme = create_tenant(&cmd, "acme", &alice).await;
    let zeta = create_tenant(&cmd, "zeta", &bob).await;
    cmd.rename_tenant(&bob, &zeta.id, "Aardvark").await.unwrap();
    cmd.suspend_tenant(&zeta.id, "unpaid").await.unwrap();

    catch_up(&executor, &pool).await;

    assert_eq!(count_tenants(&pool, None).await.unwrap(), 2);
    let rows = list_tenants(&pool, None, 0, 50).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].name, "Aardvark", "sorted by name");
    assert_eq!(rows[0].id, zeta.id);
    assert!(rows[0].suspended);
    assert_eq!(rows[0].suspended_reason.as_deref(), Some("unpaid"));
    assert_eq!(rows[0].group(), group_of(&zeta.id));
    assert_eq!(rows[1].slug, "acme");
    assert_eq!(rows[1].created_by, alice);
    assert_eq!(rows[1].member_count, 1);
    assert!(!rows[1].suspended);
    assert_eq!(
        list_tenants(&pool, Some("ZET"), 0, 50).await.unwrap().len(),
        1,
        "search is case-insensitive on slug and name"
    );
    assert_eq!(count_tenants(&pool, Some("%")).await.unwrap(), 0);

    let mine = tenants_of_user(&pool, &alice).await.unwrap();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].tenant_id, acme.id);
    assert_eq!(mine[0].slug, "acme");
    assert_eq!(mine[0].role, "owner");
    assert!(!mine[0].suspended);
    assert!(tenants_of_user(&pool, &bob).await.unwrap()[0].suspended);
    assert!(tenants_of_user(&pool, "nobody").await.unwrap().is_empty());

    let found = tenant_by_group(&pool, group_of(&acme.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.id, acme.id);
    assert!(tenant_by_group(&pool, 0xdead_beef).await.unwrap().is_none());

    // Delivering everything again changes nothing.
    read_model_subscription::<evento::Sqlite>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert_eq!(list_tenants(&pool, None, 0, 50).await.unwrap(), rows);
    assert_eq!(tenants_of_user(&pool, &alice).await.unwrap(), mine);
}
