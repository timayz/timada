use evento::{Evento, Turso};
use timada_core::Sending;
use timada_google::aggregator::{MerchantAccountIssue, MerchantProductRow};
use timada_google::{
    ACCESS_MERCHANT_CENTER, Connect, MERCHANT_ACCEPTED, MERCHANT_REFUSED, MerchantAccount,
    MerchantReport,
};
use timada_tenancy::CreateTenant;
use topcoat::mail::{Mailbox, MemoryTransport};

use super::*;
use crate::mailroom::tests::{BASE_URL, room, rows, store};

const ACCOUNT: &str = "5550001";

/// A shop whose owner has an address, connected to Merchant Center; the
/// shop's id.
async fn shop(store: &Turso) -> String {
    let evento = Evento::new(store.clone());
    evento::append("user-alice")
        .original_version(0)
        .event(&timada_identity::aggregator::EmailRegistered {
            email: "alice@example.org".to_owned(),
            label: "Alice".to_owned(),
            password_hash: String::new(),
        })
        .commit(&evento)
        .await
        .unwrap();
    let shop = timada_tenancy::Command(evento.clone())
        .create_tenant(CreateTenant {
            name: "Acme Tea".to_owned(),
            owner_user_id: "user-alice".to_owned(),
        })
        .await
        .unwrap();
    timada_identity::read_model_subscription::<Turso>(crate::db::Db::from(store.clone()))
        .ephemeral()
        .run_once(store)
        .await
        .unwrap();
    let google = Command(evento);
    google
        .connect(
            "user-alice",
            &shop.id,
            Connect {
                refresh_token_sealed: b"sealed".to_vec(),
                account: "alice@example.org".to_owned(),
                accesses: vec![ACCESS_MERCHANT_CENTER.to_owned()],
            },
        )
        .await
        .unwrap();
    google
        .choose_merchant_account(
            "user-alice",
            &shop.id,
            MerchantAccount {
                account_id: ACCOUNT.to_owned(),
                name: "Acme".to_owned(),
            },
        )
        .await
        .unwrap();
    shop.id
}

/// What Merchant Center says of the shop's two products, the ones in
/// `refused` refused, with `issues` held against the account.
async fn read(store: &Turso, shop: &str, refused: &[&str], issues: &[&str]) {
    let product = |item: &str| MerchantProductRow {
        item_id: item.to_owned(),
        ads: if refused.contains(&item) {
            MERCHANT_REFUSED
        } else {
            MERCHANT_ACCEPTED
        }
        .to_owned(),
        listings: MERCHANT_ACCEPTED.to_owned(),
        issues: Vec::new(),
    };
    Command(Evento::new(store.clone()))
        .record_merchant_read(
            "timada-dashboard",
            shop,
            ACCOUNT,
            MerchantReport {
                products: vec![product("01TREE"), product("01POST")],
                account_issues: issues
                    .iter()
                    .map(|title| MerchantAccountIssue {
                        title: (*title).to_owned(),
                        severity: "critical".to_owned(),
                        detail: String::new(),
                        documentation: String::new(),
                    })
                    .collect(),
            },
        )
        .await
        .unwrap();
}

async fn pass(store: &Turso, room: &Arc<Mailroom>) {
    subscription::<Turso>(room.clone())
        .ephemeral()
        .no_retry()
        .run_once(store)
        .await
        .expect("a pass never fails for a mail");
}

async fn run(store: &Turso, room: &Arc<Mailroom>) -> usize {
    pass(store, room).await;
    room.sweep(crate::auth::now()).await.unwrap()
}

/// Two nodes see a product turn refused; one tells the owner, once that
/// day, whatever else turns.
#[tokio::test]
async fn the_owners_are_told_once_a_day_of_what_turned_for_the_worse() {
    let store = store().await;
    let transport = MemoryTransport::new();
    let node_a = room(&store, transport.clone(), "node-a");
    let node_b = room(&store, transport.clone(), "node-b");
    let shop = shop(&store).await;

    // A first read, with a product refused from the start: there is no
    // before, so nothing turned.
    read(&store, &shop, &["01POST"], &[]).await;
    assert_eq!(run(&store, &node_a).await, 0);
    assert!(rows(&store).await.is_empty());

    // The tree is refused now, and the account has an issue.
    read(
        &store,
        &shop,
        &["01POST", "01TREE"],
        &["Store not verified"],
    )
    .await;
    assert_eq!(run(&store, &node_a).await, 1);
    assert_eq!(run(&store, &node_b).await, 0);
    let mails = transport.sent();
    assert_eq!(mails.len(), 1);
    assert_eq!(mails[0].to(), [Mailbox::new("alice@example.org").unwrap()]);
    assert_eq!(
        mails[0].subject(),
        "Merchant Center needs a look on Acme Tea"
    );
    let wire = String::from_utf8(mails[0].formatted())
        .unwrap()
        .replace("=\r\n", "");
    assert!(
        wire.contains(&merchant_link(BASE_URL, "acme-tea")),
        "{wire}"
    );
    // What the shop's feed says now: both refused, one issue.
    assert!(
        wire.contains("2 products that Google showed are now refused."),
        "{wire}"
    );
    assert!(
        wire.contains("a new issue against the Merchant Center account"),
        "{wire}"
    );
    let day = merchant_notice_day(crate::auth::now());
    let notice = Command(Evento::new(store.clone()))
        .load_merchant_notice(&shop, &day)
        .await
        .unwrap();
    // A day that turned between the read and here has its own notice;
    // either way the one that was owed went out.
    assert!(notice.is_none_or(|notice| notice.sending == Sending::Sent));
    pass(&store, &node_a).await;
    pass(&store, &node_b).await;
    assert!(rows(&store).await.is_empty());

    // Put right, then refused again the same day: the day was told.
    read(&store, &shop, &[], &[]).await;
    read(&store, &shop, &["01TREE"], &[]).await;
    assert_eq!(run(&store, &node_a).await, 0);
    assert_eq!(run(&store, &node_b).await, 0);
    assert_eq!(transport.sent().len(), 1);
    assert!(rows(&store).await.is_empty());
}

/// What was put right before the mail left is not told.
#[tokio::test]
async fn a_notice_is_dropped_once_nothing_is_held_any_more() {
    let store = store().await;
    let transport = MemoryTransport::new();
    let node = room(&store, transport.clone(), "node-a");
    let shop = shop(&store).await;
    read(&store, &shop, &[], &[]).await;
    read(&store, &shop, &["01TREE"], &[]).await;
    read(&store, &shop, &[], &[]).await;
    assert_eq!(run(&store, &node).await, 0);
    assert!(transport.sent().is_empty());
    assert!(rows(&store).await.is_empty());
}
