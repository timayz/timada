mod common;

use common::*;
use timada_core::page::PageFrom;
use timada_core::{Claimant, MailStatus};
use timada_orders::{
    Command, OrderFilter, OrderStatus, cart_refused_returns, count_open_carts, count_orders,
    fulfilled_between, hold_reminders_due, open_carts, order_count, order_mails, order_state,
    orders_of_email, orders_of_tenant, payments_due, product_ever_ordered, read_model_subscription,
};
use timada_orders::{HOLD_REMINDER_BEFORE_SECS, ORDER_MAIL_WINDOW_SECS};

#[tokio::test]
async fn orders_and_carts_are_listed() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;

    let left = cart_with(&orders, &mug, 3).await;
    let _emptied = emptied_cart(&orders, &mug).await;
    let ordered = cart_with(&orders, &mug, 2).await;
    let placed = pay(&orders, &ordered, checkout()).await;
    let cancelled_cart = cart_with(&orders, &mug, 1).await;
    let cancelled = pay(&orders, &cancelled_cart, checkout()).await;
    cancel(&orders, &cancelled.order_id, "Test").await;
    let held_cart = cart_with(&orders, &mug, 1).await;
    let held = place(&orders, &held_cart, checkout()).await;

    // Twice: delivery may repeat without the rows drifting.
    for _ in 0..2 {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    }
    let all = OrderFilter::default();

    let rows = orders_of_tenant(&pool, TENANT, &all, PageFrom::First, 10)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        order_count(&pool, TENANT).await.unwrap(),
        3,
        "the home's count"
    );
    let row = rows.iter().find(|row| row.id == placed.order_id).unwrap();
    assert_eq!(row.reference, placed.reference);
    assert_eq!(row.status(), OrderStatus::Paid);
    assert_eq!(row.payment_state, "paid");
    assert!(row.paid_at.is_some());
    assert!(row.payment_id.starts_with("pi_"));
    assert_eq!(row.total_label(), "25.00 €");
    assert_eq!(row.item_count, 2);
    assert_eq!(row.name, "Ada Lovelace");
    let held_row = rows.iter().find(|row| row.id == held.order_id).unwrap();
    assert_eq!(held_row.status(), OrderStatus::Authorized);
    assert_eq!(held_row.payment_state, "held");
    assert_eq!(held_row.paid_at, None);
    let cancelled_row = rows
        .iter()
        .find(|row| row.id == cancelled.order_id)
        .unwrap();
    assert_eq!(cancelled_row.payment_state, "paid", "taken, then refunded");

    // Two at a time: the second page goes on where the first stopped, and
    // the page above it is the first again.
    let first = orders_of_tenant(&pool, TENANT, &all, PageFrom::First, 2)
        .await
        .unwrap();
    let second = orders_of_tenant(
        &pool,
        TENANT,
        &all,
        PageFrom::Older(first.older.as_ref().expect("more below")),
        2,
    )
    .await
    .unwrap();
    assert_eq!(second.rows.len(), 1);
    assert!(second.older.is_none());
    assert!(first.rows.iter().all(|row| row.id != second.rows[0].id));
    let back = orders_of_tenant(
        &pool,
        TENANT,
        &all,
        PageFrom::Newer(second.newer.as_ref().expect("a page above")),
        2,
    )
    .await
    .unwrap();
    assert_eq!(back, first);

    let only_cancelled = OrderFilter::default().status(Some(OrderStatus::Cancelled));
    let rows = orders_of_tenant(&pool, TENANT, &only_cancelled, PageFrom::First, 10)
        .await
        .unwrap()
        .rows;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, cancelled.order_id);

    let by_reference = OrderFilter::default().search(Some(&placed.reference));
    assert_eq!(count_orders(&pool, TENANT, &by_reference).await.unwrap(), 1);
    let by_email = OrderFilter::default().search(Some("ADA@example"));
    assert_eq!(count_orders(&pool, TENANT, &by_email).await.unwrap(), 3);
    // The name it ships to, accents and case aside; a search too short for
    // the index still finds the rows; another tenant's search finds none.
    let by_name = OrderFilter::default().search(Some("LOVÉLACE"));
    assert_eq!(count_orders(&pool, TENANT, &by_name).await.unwrap(), 3);
    let short = OrderFilter::default().search(Some("da"));
    assert_eq!(count_orders(&pool, TENANT, &short).await.unwrap(), 3);
    assert_eq!(count_orders(&pool, "other", &by_name).await.unwrap(), 0);
    let of_email = orders_of_email(&pool, TENANT, &checkout().email.to_lowercase(), 10)
        .await
        .unwrap();
    assert_eq!(of_email.len(), 3);
    assert!(
        orders_of_email(&pool, TENANT, "bob@example.com", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        orders_of_email(&pool, "tenant-2", &of_email[0].email, 10)
            .await
            .unwrap()
            .is_empty()
    );

    let carts = open_carts(&pool, TENANT, 0, 10).await.unwrap();
    assert_eq!(
        carts.len(),
        1,
        "an empty or ordered cart is not left behind"
    );
    assert_eq!(carts[0].id, left);
    assert_eq!(count_open_carts(&pool, TENANT).await.unwrap(), 1);

    // Which products were ever ordered: a cancelled order counts, a cart
    // does not, and nothing crosses tenants.
    assert!(product_ever_ordered(&pool, TENANT, &mug).await.unwrap());
    assert!(!product_ever_ordered(&pool, "tenant-2", &mug).await.unwrap());
    let in_a_cart_only = product(&catalog, "Plate", 500, "EUR").await;
    cart_with(&orders, &in_a_cart_only, 1).await;
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();
    assert!(
        !product_ever_ordered(&pool, TENANT, &in_a_cart_only)
            .await
            .unwrap()
    );
    assert_eq!(carts[0].item_count, 3);
}

#[tokio::test]
async fn fulfilled_orders_are_found_by_when_they_left() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let paid = pay(&orders, &cart_with(&orders, &mug, 1).await, checkout()).await;
    let left = pay(&orders, &cart_with(&orders, &mug, 1).await, checkout()).await;
    orders
        .fulfil_order(ALICE, &left.order_id, None, None)
        .await
        .unwrap();
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&executor)
        .await
        .unwrap();

    let fulfilled_at = orders
        .load_order(&left.order_id)
        .await
        .unwrap()
        .unwrap()
        .fulfilled_at
        .unwrap() as i64;
    let found = fulfilled_between(&pool, fulfilled_at, fulfilled_at + 1)
        .await
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].id, left.order_id);
    assert_eq!(found[0].tenant_id, TENANT);
    assert_eq!(found[0].fulfilled_at, fulfilled_at);
    assert!(
        fulfilled_between(&pool, fulfilled_at + 1, fulfilled_at + 100)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(found.iter().all(|order| order.id != paid.order_id));
}

/// A hold the provider dropped while the supplier may have the order leaves
/// nothing to settle with the provider: the sweep passes it over until the
/// merchant asks to cancel it.
#[tokio::test]
async fn an_order_kept_open_without_its_hold_waits_for_the_merchant() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    // Two holds whose time ran out at 100,000.
    let lapsed = async || {
        let cart_id = cart_with(&orders, &mug, 1).await;
        let (_, mut payment) = open_payment(&orders, &cart_id, checkout()).await;
        payment.capture_before = Some(100_000);
        orders
            .place_order(DASHBOARD, &cart_id, payment)
            .await
            .unwrap()
            .order_id
    };
    let unread = lapsed().await;
    let kept = lapsed().await;
    orders
        .begin_supplier_order(DASHBOARD, &kept, "aliexpress", 0)
        .await
        .unwrap();
    let payment_id = orders
        .load_order(&kept)
        .await
        .unwrap()
        .unwrap()
        .payment
        .payment_id;
    orders
        .record_release(DASHBOARD, &kept, &payment_id, "")
        .await
        .unwrap();
    let due = async || {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
        let mut ids: Vec<String> = payments_due(&pool, 200_000, 200_000, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
        ids.sort();
        ids
    };

    assert_eq!(due().await, std::slice::from_ref(&unread));

    // Settled outside timada: paid, with nothing for the sweep either.
    let settled = lapsed().await;
    orders
        .begin_supplier_order(DASHBOARD, &settled, "aliexpress", 0)
        .await
        .unwrap();
    let payment_id = orders
        .load_order(&settled)
        .await
        .unwrap()
        .unwrap()
        .payment
        .payment_id;
    orders
        .record_release(DASHBOARD, &settled, &payment_id, "")
        .await
        .unwrap();
    orders.settle_outside(ALICE, &settled, None).await.unwrap();
    assert_eq!(due().await, std::slice::from_ref(&unread));
    let rows = orders_of_tenant(&pool, TENANT, &OrderFilter::default(), PageFrom::First, 10)
        .await
        .unwrap()
        .rows;
    let row = rows.iter().find(|row| row.id == settled).unwrap();
    assert_eq!(row.status(), OrderStatus::Paid);
    assert_eq!(row.payment_state, "settled_outside");
    assert!(row.paid_at.is_some());

    orders
        .request_cancellation(ALICE, &kept, "Cancelled on AliExpress".into(), true)
        .await
        .unwrap();
    let mut both = vec![unread, kept];
    both.sort();
    assert_eq!(due().await, both);
}

/// The sweep that reminds merchants finds, in the read model, the orders
/// whose money is still only held while the supplier may have them, once
/// their time came, and no longer once reminded, or settled either way.
#[tokio::test]
async fn orders_due_a_hold_reminder_are_listed_until_reminded() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    // Holds the provider said lapse at 1,000,000.
    let dated = async || {
        let cart_id = cart_with(&orders, &mug, 1).await;
        let (_, mut payment) = open_payment(&orders, &cart_id, checkout()).await;
        payment.capture_before = Some(1_000_000);
        orders
            .place_order(DASHBOARD, &cart_id, payment)
            .await
            .unwrap()
            .order_id
    };
    let at_risk = dated().await;
    let paid = dated().await;
    let untouched = dated().await;
    for id in [&at_risk, &paid] {
        orders
            .begin_supplier_order(DASHBOARD, id, "aliexpress", 0)
            .await
            .unwrap();
    }
    capture(&orders, &paid).await;
    let due_at = async |now: i64| {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
        hold_reminders_due(&pool, now, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect::<Vec<_>>()
    };
    let reminder = (1_000_000 - HOLD_REMINDER_BEFORE_SECS) as i64;

    assert_eq!(due_at(reminder - 1).await, Vec::<String>::new());
    assert_eq!(due_at(reminder).await, vec![at_risk.clone()]);
    let _ = untouched;

    assert!(
        orders
            .claim_hold_reminder(DASHBOARD, &at_risk, reminder as u64)
            .await
            .unwrap()
    );
    assert_eq!(due_at(reminder).await, Vec::<String>::new());
}

/// The order's page reads the read model: the row carries the order whole,
/// the facts a storefront is never answered included, with the mails the
/// shopper was sent about it and the payments of its cart the provider
/// would not give back.
#[tokio::test]
async fn an_orders_row_carries_the_order_whole_with_its_mails_and_refused_returns() {
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let cart_id = cart_with(&orders, &mug, 1).await;
    // Two payments opened for one cart: the order takes the first, the
    // second is asked back and the provider will not give it.
    let (_, first) = open_payment(&orders, &cart_id, checkout()).await;
    let (_, second) = open_payment(&orders, &cart_id, checkout()).await;
    let placed = orders
        .place_order(DASHBOARD, &cart_id, first)
        .await
        .unwrap();
    capture(&orders, &placed.order_id).await;
    orders
        .request_payment_return(DASHBOARD, &cart_id, &second.payment_id, "duplicate_payment")
        .await
        .unwrap();
    orders
        .refuse_payment_return(
            DASHBOARD,
            &cart_id,
            &second.payment_id,
            "charge already captured",
        )
        .await
        .unwrap();
    orders
        .fulfil_order(ALICE, &placed.order_id, Some("6A1".to_owned()), None)
        .await
        .unwrap();
    let now = 1_700_000_000;
    assert!(
        orders
            .claim_order_mail(
                &placed.order_id,
                "order_confirmation",
                now + ORDER_MAIL_WINDOW_SECS,
                Claimant {
                    node: DASHBOARD,
                    now,
                },
            )
            .await
            .unwrap()
    );
    orders
        .record_order_mail_sent(&placed.order_id, "order_confirmation")
        .await
        .unwrap();
    assert!(
        orders
            .claim_order_mail(
                &placed.order_id,
                "order_cancelled",
                now + ORDER_MAIL_WINDOW_SECS,
                Claimant {
                    node: DASHBOARD,
                    now,
                },
            )
            .await
            .unwrap()
    );
    orders
        .record_order_mail_not_sent(
            &placed.order_id,
            "order_cancelled",
            DASHBOARD,
            "connection refused",
            now,
        )
        .await
        .unwrap();

    for _ in 0..2 {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    }

    let shown = order_state(&pool, TENANT, &placed.order_id)
        .await
        .unwrap()
        .unwrap();
    let exact = orders.load_order(&placed.order_id).await.unwrap().unwrap();
    assert_eq!(
        shown.fulfilled_by.as_deref(),
        Some(ALICE),
        "a shop-only fact"
    );
    assert_eq!(shown.tracking.as_deref(), Some("6A1"));
    assert_eq!(shown, exact);
    assert_eq!(
        order_state(&pool, "tenant-2", &placed.order_id)
            .await
            .unwrap(),
        None
    );

    let mails = order_mails(&pool, TENANT, &placed.order_id).await.unwrap();
    assert_eq!(mails.len(), 2);
    let sent = mails
        .iter()
        .find(|m| m.mail == "order_confirmation")
        .unwrap();
    assert_eq!(sent.status(), MailStatus::Sent);
    assert!(sent.sent_at.is_some_and(|at| at > 0));
    let retrying = mails.iter().find(|m| m.mail == "order_cancelled").unwrap();
    assert_eq!(retrying.status(), MailStatus::Retrying);
    assert_eq!((retrying.attempts, retrying.sent_at), (1, None));
    assert_eq!(retrying.reason, "connection refused");

    assert_eq!(
        cart_refused_returns(&pool, TENANT, &cart_id).await.unwrap(),
        vec![(second.payment_id, "charge already captured".to_owned())]
    );
}

#[tokio::test]
async fn a_dropped_cart_leaves_the_carts_table_and_a_replay_leaves_it_out() {
    const IDLE: u64 = 30 * 24 * 60 * 60;
    let (executor, pool) = executor_with_pool().await;
    let orders = Command::new(executor.clone(), TENANT);
    let catalog = timada_catalog::Command::new(executor.clone(), TENANT);
    let mug = product(&catalog, "Mug", 1250, "EUR").await;
    let kept = cart_with(&orders, &mug, 1).await;
    let left = cart_with(&orders, &mug, 2).await;
    let follow = || async {
        read_model_subscription::<evento::Turso>(pool.clone())
            .run_once(&executor)
            .await
            .unwrap();
    };
    let listed = || async {
        let mut ids: Vec<String> = open_carts(&pool, TENANT, 0, 10)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.id)
            .collect();
        ids.sort();
        ids
    };
    follow().await;
    let mut both = vec![kept.clone(), left.clone()];
    both.sort();
    assert_eq!(listed().await, both);

    let touched = orders.load_cart(&left).await.unwrap().unwrap().updated_at;
    let dropped = orders
        .expire_cart(DASHBOARD, &left, touched + IDLE, IDLE)
        .await
        .unwrap();
    assert!(dropped);
    follow().await;
    assert_eq!(listed().await, vec![kept.clone()]);
    assert_eq!(count_open_carts(&pool, TENANT).await.unwrap(), 1);

    // A rebuild reads every event again into an empty table: the dropped
    // cart's first events do not put its row back on the way.
    pool.execute("DELETE FROM orders_carts", ()).await.unwrap();
    pool.execute("DELETE FROM orders_tallies", ())
        .await
        .unwrap();
    pool.execute("DELETE FROM subscriber", ()).await.unwrap();
    follow().await;
    assert_eq!(listed().await, vec![kept]);
    let rows: i64 = pool
        .fetch_scalar("SELECT count(*) FROM orders_carts", ())
        .await
        .unwrap();
    assert_eq!(rows, 1);
    assert_eq!(count_open_carts(&pool, TENANT).await.unwrap(), 1);
}
