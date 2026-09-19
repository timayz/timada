//! The outbox on its own (idempotent queuing, retries, giving up), then the
//! whole chain: facts of the other contexts → queued e-mails → a transport.

use std::sync::atomic::{AtomicU32, Ordering};

use timada_core::{Address, Civility, Money};
use timada_mailer::{
    Content, DeliveryPolicy, Email, LogTransport, MAX_ATTEMPTS, MailError, MailerConfig,
    MailerTemplates, MemoryTransport, OutboxStatus, SendFuture, Templates, Transport, count_outbox,
    deliver_pending, deliver_pending_with, enqueue, list_outbox, load_outbox_message,
    mailer_subscription, migrations, retry,
};
use timada_order::{DeliveryChoice, OrderLine, PaymentMode, PlaceOrder, Seller};

fn config() -> MailerConfig {
    MailerConfig {
        from: "Timada <no-reply@timada.example>".into(),
        shop_name: "Timada".into(),
        base_url: "https://shop.example/".into(),
        returns_address: "Timada — Service retours\n1 rue de l'Entrepôt\n31000 Toulouse".into(),
        max_event_age_secs: MailerConfig::DEFAULT_MAX_EVENT_AGE_SECS,
    }
}

fn email(to: &str) -> Email {
    Email {
        from: "Timada <no-reply@timada.example>".into(),
        to: to.into(),
        subject: "Bonjour".into(),
        body: "Un message.".into(),
        html_body: None,
    }
}

/// Fails its first `failures` sends, then delivers.
struct Flaky {
    failures: u32,
    calls: AtomicU32,
}

impl Transport for Flaky {
    fn send<'a>(&'a self, _email: &'a Email) -> SendFuture<'a> {
        Box::pin(async move {
            if self.calls.fetch_add(1, Ordering::SeqCst) < self.failures {
                return Err(MailError::Transport("relay unavailable".into()));
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn the_outbox_queues_once_retries_and_gives_up() -> anyhow::Result<()> {
    let (_executor, db) = timada_core::testing::memory_executor(migrations()).await?;

    // The same message id is only ever queued once.
    assert!(enqueue(&db, "m-1", "test", &email("ada@example.com")).await?);
    assert!(!enqueue(&db, "m-1", "test", &email("ada@example.com")).await?);
    assert_eq!(count_outbox(&db, None).await?, 1);
    assert_eq!(count_outbox(&db, Some(OutboxStatus::Pending)).await?, 1);

    // A failing relay leaves the e-mail pending; the next pass delivers it,
    // and a delivered e-mail is never sent again.
    let flaky = Flaky {
        failures: 1,
        calls: AtomicU32::new(0),
    };
    let at_once = DeliveryPolicy::without_delays();
    let first = deliver_pending_with(&db, &flaky, &at_once).await?;
    assert_eq!((first.sent, first.failed), (0, 1));
    let row = load_outbox_message(&db, "m-1")
        .await?
        .ok_or_else(|| anyhow::anyhow!("message missing"))?;
    assert_eq!(row.status(), OutboxStatus::Pending);
    assert_eq!(
        row.last_error.as_deref(),
        Some("transport failure: relay unavailable")
    );
    let second = deliver_pending_with(&db, &flaky, &at_once).await?;
    assert_eq!((second.sent, second.failed), (1, 0));
    let third = deliver_pending_with(&db, &flaky, &at_once).await?;
    assert_eq!((third.sent, third.failed), (0, 0));
    assert_eq!(count_outbox(&db, Some(OutboxStatus::Sent)).await?, 1);

    // A relay that never recovers: given up on after MAX_ATTEMPTS, until an
    // operator asks for a retry.
    enqueue(&db, "m-2", "test", &email("bob@example.com")).await?;
    let dead = Flaky {
        failures: u32::MAX,
        calls: AtomicU32::new(0),
    };
    for _ in 0..MAX_ATTEMPTS + 2 {
        deliver_pending_with(&db, &dead, &at_once).await?;
    }
    assert_eq!(dead.calls.load(Ordering::SeqCst), MAX_ATTEMPTS as u32);
    let failed = list_outbox(&db, Some(OutboxStatus::Failed), 10, 0).await?;
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].message_id, "m-2");
    retry(&db, "m-2").await?;
    let sent = deliver_pending(&db, &LogTransport).await?;
    assert_eq!(sent.sent, 1);
    assert_eq!(count_outbox(&db, Some(OutboxStatus::Failed)).await?, 0);
    Ok(())
}

#[tokio::test]
async fn a_failed_email_waits_for_its_next_attempt() -> anyhow::Result<()> {
    let (_executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    enqueue(&db, "m-1", "test", &email("ada@example.com")).await?;
    let dead = Flaky {
        failures: u32::MAX,
        calls: AtomicU32::new(0),
    };

    // The default schedule waits a minute after the first failure: passes in
    // between leave the e-mail alone instead of burning its attempts.
    let first = deliver_pending(&db, &dead).await?;
    assert_eq!(first.failed, 1);
    for _ in 0..3 {
        let idle = deliver_pending(&db, &dead).await?;
        assert_eq!((idle.sent, idle.failed), (0, 0));
    }
    assert_eq!(dead.calls.load(Ordering::SeqCst), 1);
    let row = load_outbox_message(&db, "m-1")
        .await?
        .ok_or_else(|| anyhow::anyhow!("message missing"))?;
    assert_eq!(row.attempts, 1);
    assert_eq!(row.status(), OutboxStatus::Pending);
    let now = timada_core::time::now_unix_secs()? as i64;
    assert!((55..=65).contains(&(row.next_attempt_at - now)), "{row:?}");

    // An operator's retry sends it now.
    retry(&db, "m-1").await?;
    assert_eq!(deliver_pending(&db, &LogTransport).await?.sent, 1);
    Ok(())
}

/// Takes its time, so that two passes overlap.
#[derive(Default)]
struct Slow {
    sent: std::sync::Mutex<Vec<String>>,
}

impl Transport for Slow {
    fn send<'a>(&'a self, email: &'a Email) -> SendFuture<'a> {
        Box::pin(async move {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            self.sent
                .lock()
                .map_err(|_| MailError::Transport("poisoned".into()))?
                .push(email.to.clone());
            Ok(())
        })
    }
}

#[tokio::test]
async fn workers_side_by_side_never_send_an_email_twice() -> anyhow::Result<()> {
    let (_executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    for index in 0..12 {
        enqueue(
            &db,
            &format!("m-{index}"),
            "test",
            &email(&format!("shopper-{index}@example.com")),
        )
        .await?;
    }
    // Small batches, so every worker gets some of the queue.
    let policy = DeliveryPolicy {
        batch: 3,
        ..DeliveryPolicy::default()
    };
    let transport = Slow::default();
    let worker = || async {
        let mut sent = 0;
        loop {
            let pass = deliver_pending_with(&db, &transport, &policy).await?;
            if pass.sent == 0 {
                return anyhow::Ok(sent);
            }
            sent += pass.sent;
        }
    };
    let (a, b, c) = tokio::try_join!(worker(), worker(), worker())?;
    assert_eq!(a + b + c, 12);

    let mut recipients = transport
        .sent
        .lock()
        .map_err(|_| anyhow::anyhow!("poisoned"))?
        .clone();
    recipients.sort();
    let before = recipients.len();
    recipients.dedup();
    assert_eq!(
        (before, recipients.len()),
        (12, 12),
        "an e-mail went out twice"
    );
    assert_eq!(count_outbox(&db, Some(OutboxStatus::Sent)).await?, 12);
    Ok(())
}

#[tokio::test]
async fn a_crashed_workers_rows_are_taken_over_when_the_lease_expires() -> anyhow::Result<()> {
    let (_executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    enqueue(&db, "m-1", "test", &email("ada@example.com")).await?;
    // A worker claimed the row and died before sending it.
    let now = timada_core::time::now_unix_secs()? as i64;
    sqlx::query("UPDATE mailer_outbox SET claimed_by = 'dead-worker', claimed_until = ?")
        .bind(now + 300)
        .execute(&db)
        .await?;
    assert_eq!(deliver_pending(&db, &LogTransport).await?.sent, 0);

    sqlx::query("UPDATE mailer_outbox SET claimed_until = ?")
        .bind(now - 1)
        .execute(&db)
        .await?;
    assert_eq!(deliver_pending(&db, &LogTransport).await?.sent, 1);
    Ok(())
}

/// A host's own words for one e-mail, with an HTML alternative; every other
/// e-mail keeps the built-in text.
struct EnglishWelcome;

impl Templates for EnglishWelcome {
    fn welcome(&self, config: &MailerConfig, first_name: &str) -> Content {
        Content::text(
            format!("Welcome to {}", config.shop_name),
            format!("Hello {first_name}, your account is ready."),
        )
        .with_html(format!(
            "<p>Hello <strong>{first_name}</strong>, your account is ready.</p>"
        ))
    }
}

#[tokio::test]
async fn a_host_words_its_own_emails() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    timada_customer::Command(&executor)
        .register_customer(timada_customer::RegisterCustomer {
            email: "ada@example.com".into(),
            civility: Civility::Mrs,
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
        })
        .await?;
    mailer_subscription()
        .data(db.clone())
        .data(config())
        .data(MailerTemplates::new(EnglishWelcome))
        .run_once(&executor)
        .await?;
    let outbox = MemoryTransport::default();
    deliver_pending(&db, &outbox).await?;

    let sent = outbox.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].subject, "Welcome to Timada");
    assert_eq!(sent[0].body, "Hello Ada, your account is ready.");
    assert!(
        sent[0]
            .html_body
            .as_deref()
            .is_some_and(|html| html.contains("<strong>Ada</strong>"))
    );
    let stored = list_outbox(&db, None, 10, 0).await?;
    assert_eq!(stored[0].kind, "welcome");
    assert!(stored[0].html_body.is_some());
    Ok(())
}

fn address() -> Address {
    Address {
        first_name: "Ada".into(),
        last_name: "Lovelace".into(),
        line1: "12 rue des Machines".into(),
        postal_code: "31000".into(),
        city: "Toulouse".into(),
        country_code: "FR".into(),
        ..Address::default()
    }
}

#[tokio::test]
async fn facts_of_the_other_contexts_become_emails() -> anyhow::Result<()> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    let outbox = MemoryTransport::default();
    let sync = || async {
        for _ in 0..2 {
            mailer_subscription()
                .data(db.clone())
                .data(config())
                .run_once(&executor)
                .await?;
        }
        deliver_pending(&db, &outbox).await?;
        anyhow::Ok(())
    };

    let customer_id = timada_customer::Command(&executor)
        .register_customer(timada_customer::RegisterCustomer {
            email: "ada@example.com".into(),
            civility: Civility::Mrs,
            first_name: "Ada".into(),
            last_name: "Lovelace".into(),
        })
        .await?;
    let product_id = timada_catalog::Command(&executor)
        .create_product(timada_catalog::CreateProduct {
            sku: "AOC-24G4XE".into(),
            name: "AOC 24G4XE".into(),
            brand: timada_catalog::Brand {
                name: "AOC".into(),
                slug: "aoc".into(),
            },
            category_path: vec!["Écrans".into()],
            short_description: "Écran 24 pouces".into(),
            warranty_months: 36,
        })
        .await?;

    // Order placed → confirmation, with the number, the lines and a link.
    let orders = timada_order::Command(&executor);
    let order_id = orders
        .place_order(PlaceOrder {
            cart_id: "cart-1".into(),
            customer_id: customer_id.clone(),
            seller: Seller::Ldlc,
            lines: vec![OrderLine {
                product_id: product_id.clone(),
                name: "AOC 24G4XE".into(),
                quantity: 2,
                unit_price: Money::eur(11_995),
                warranty_months: 36,
            }],
            delivery_address: address(),
            billing_address: address(),
            delivery: DeliveryChoice {
                method_code: "colissimo".into(),
                pickup_store_id: None,
            },
            payment_mode: PaymentMode::Card,
            shipping_fee: Money::eur(590),
            handling_fee: Money::eur(0),
            promo_code: None,
            discount: None,
            order_number: Some("C2026-000042".into()),
            tax: None,
        })
        .await?;
    sync().await?;
    // Registering already wrote the welcome; the confirmation is the second.
    let sent = outbox.sent();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0].subject, "Bienvenue chez Timada");
    let sent = &sent[1..];
    assert_eq!(sent[0].to, "ada@example.com");
    assert_eq!(sent[0].from, "Timada <no-reply@timada.example>");
    assert_eq!(
        sent[0].subject,
        "Confirmation de votre commande C2026-000042"
    );
    assert!(sent[0].body.starts_with("Bonjour Ada,"), "{}", sent[0].body);
    assert!(sent[0].body.contains("2 × AOC 24G4XE"), "{}", sent[0].body);
    assert!(sent[0].body.contains("245,80 €"), "{}", sent[0].body);
    assert!(
        sent[0]
            .body
            .contains(&format!("https://shop.example/account/orders/{order_id}")),
        "{}",
        sent[0].body
    );

    // Redelivering the same events writes nothing more.
    sync().await?;
    assert_eq!(outbox.sent().len(), 2);

    // Resent on request, refunded, cancelled: one e-mail each.
    orders.resend_confirmation(&order_id).await?;
    let payments = timada_payment::Command(&executor);
    let payment_id = payments
        .request_payment(timada_payment::RequestPayment {
            order_id: order_id.clone(),
            amount: Money::eur(24_580),
            method: timada_payment::PaymentMethod::Card,
        })
        .await?;
    payments
        .capture_payment(&payment_id, "psp-1".into())
        .await?;
    payments
        .refund_payment(&payment_id, Money::eur(2_000), "geste commercial".into())
        .await?;
    orders
        .cancel_order(&order_id, "rupture fournisseur")
        .await?;
    sync().await?;
    let subjects: Vec<String> = outbox.sent().into_iter().map(|e| e.subject).collect();
    assert_eq!(subjects.len(), 5, "{subjects:?}");
    assert_eq!(
        subjects
            .iter()
            .filter(|s| s.starts_with("Confirmation de votre commande"))
            .count(),
        2
    );
    assert!(
        subjects.contains(&"Remboursement de 20,00 € sur votre commande C2026-000042".to_owned()),
        "{subjects:?}"
    );
    assert!(
        subjects.contains(&"Votre commande C2026-000042 a été annulée".to_owned()),
        "{subjects:?}"
    );

    // Back in stock → to the address left with the alert, not the account's.
    let inventory = timada_inventory::Command(&executor);
    let alert = inventory
        .request_back_in_stock_alert(timada_inventory::RequestBackInStockAlert {
            product_id: product_id.clone(),
            customer_id: customer_id.clone(),
            email: "ada.perso@example.com".into(),
        })
        .await?;
    inventory.trigger_back_in_stock_alert(&alert).await?;

    // A question answered by the shop → to whoever asked.
    let reviews = timada_review::Command(&executor);
    let question = reviews
        .ask_question(timada_review::AskQuestion {
            product_id: product_id.clone(),
            customer_id: customer_id.clone(),
            body: "Compatible G-SYNC ?".into(),
        })
        .await?;
    reviews
        .answer_question(
            &question,
            timada_review::AnswerAuthor::Staff,
            "Oui, G-SYNC Compatible.".into(),
        )
        .await?;
    // Answering one's own question is not news.
    reviews
        .answer_question(
            &question,
            timada_review::AnswerAuthor::Customer {
                customer_id: customer_id.clone(),
            },
            "Merci !".into(),
        )
        .await?;
    // A review through moderation, another refused, a question refused.
    let kept = reviews
        .submit_review(timada_review::SubmitReview {
            product_id: product_id.clone(),
            customer_id: customer_id.clone(),
            order_id: None,
            rating: 5,
            title: String::new(),
            body: "Parfait.".into(),
        })
        .await?;
    reviews.publish_review(&kept).await?;
    let spam = reviews
        .ask_question(timada_review::AskQuestion {
            product_id: product_id.clone(),
            customer_id: customer_id.clone(),
            body: "Achetez mes cryptos".into(),
        })
        .await?;
    reviews.reject_question(&spam, "Publicité".into()).await?;
    sync().await?;
    let sent = outbox.sent();
    assert_eq!(sent.len(), 9);
    assert!(
        sent.iter()
            .any(|e| e.subject == "Votre avis sur AOC 24G4XE est en ligne"),
        "{sent:?}"
    );
    let refused = sent
        .iter()
        .find(|e| e.subject == "Votre question sur AOC 24G4XE n'a pas été publiée")
        .ok_or_else(|| anyhow::anyhow!("no refused-question e-mail"))?;
    assert!(refused.body.contains("Publicité"), "{}", refused.body);
    let back = sent
        .iter()
        .find(|e| e.subject == "AOC 24G4XE est de nouveau disponible")
        .ok_or_else(|| anyhow::anyhow!("no back-in-stock e-mail"))?;
    assert_eq!(back.to, "ada.perso@example.com");
    assert!(back.body.starts_with("Bonjour,"), "{}", back.body);
    assert!(
        back.body
            .contains(&format!("https://shop.example/p/{product_id}")),
        "{}",
        back.body
    );
    let answered = sent
        .iter()
        .find(|e| e.subject == "Une réponse à votre question sur AOC 24G4XE")
        .ok_or_else(|| anyhow::anyhow!("no answer e-mail"))?;
    assert!(
        answered.body.contains("Compatible G-SYNC ?"),
        "{}",
        answered.body
    );
    assert!(
        answered.body.contains("Oui, G-SYNC Compatible."),
        "{}",
        answered.body
    );
    Ok(())
}

#[tokio::test]
async fn old_events_are_not_emailed_about() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let inventory = timada_inventory::Command(&executor);
    let alert = inventory
        .request_back_in_stock_alert(timada_inventory::RequestBackInStockAlert {
            product_id: "p-1".into(),
            customer_id: "c-1".into(),
            email: "ada@example.com".into(),
        })
        .await?;
    inventory.trigger_back_in_stock_alert(&alert).await?;

    // A mailer plugged into a shop with history: nothing older than the
    // window is written about. A window of 0 lets through the events of this
    // very second only, so let that second pass first.
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    mailer_subscription()
        .data(db.clone())
        .data(MailerConfig {
            max_event_age_secs: 0,
            ..config()
        })
        .run_once(&executor)
        .await?;
    assert_eq!(count_outbox(&db, None).await?, 0);
    Ok(())
}
