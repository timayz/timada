//! Socket-free tests through `Router::handle`: auth, mounting under a custom
//! segment, the orders, promotions, inventory, invoices, refunds, returns,
//! reviews, questions and e-mails sections, branded 404s.

use timada_admin::{AdminConfig, AdminServices, Stylesheet, create_admin, migrations};
use timada_core::{Address, Money};
use timada_order::{
    OrderLine, OrderStatus, PlaceOrder, load_order_details, order_history_subscription,
};
use topcoat::{
    asset::{AssetCatalog, AssetConfig},
    router::{Body, Router, StatusCode, request::Request, response::Response, to_bytes},
};

const EMAIL: &str = "admin@timada.example";
const PASSWORD: &str = "correct horse battery staple";

struct Harness {
    router: Router,
    executor: evento::Sqlite,
    db: sqlx::SqlitePool,
}

async fn harness(mount: &str) -> anyhow::Result<Harness> {
    let mut all = migrations();
    all.extend(timada_order::migrations());
    all.extend(timada_catalog::migrations());
    all.extend(timada_customer::migrations());
    all.extend(timada_promotion::migrations());
    all.extend(timada_inventory::migrations());
    all.extend(timada_invoice::migrations());
    all.extend(timada_payment::migrations());
    all.extend(timada_review::migrations());
    all.extend(timada_mailer::migrations());
    all.extend(timada_returns::migrations());
    let (executor, db) = timada_core::testing::memory_executor(all).await?;
    create_admin(&db, EMAIL, PASSWORD).await?;

    let router = timada_admin::router(
        AdminConfig {
            mount: mount.into(),
            stylesheet: Stylesheet::Url("/dev.css".into()),
        },
        AssetConfig::hosted_at("/assets", AssetCatalog::default()),
        AdminServices::new(executor.clone(), db.clone()),
    );
    Ok(Harness {
        router,
        executor,
        db,
    })
}

fn get(uri: &str, cookie: Option<&str>) -> Request {
    let mut builder = http::Request::builder().uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder.body(Body::empty()).unwrap_or_default()
}

fn post(uri: &str, form: &str, cookie: Option<&str>) -> Request {
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("sec-fetch-site", "same-origin");
    if let Some(cookie) = cookie {
        builder = builder.header("cookie", cookie);
    }
    builder
        .body(Body::from(form.to_owned()))
        .unwrap_or_default()
}

async fn text(response: Response) -> anyhow::Result<String> {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn location(response: &Response) -> String {
    response
        .headers()
        .get("location")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// The `name=value` pair of the session cookie set by a login response.
fn session_cookie(response: &Response) -> Option<String> {
    response
        .headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find(|v| v.contains("timada_admin"))
        .and_then(|v| v.split(';').next())
        .map(str::to_owned)
}

async fn sign_in(h: &Harness, mount: &str) -> anyhow::Result<String> {
    let form = format!("email={EMAIL}&password={}", PASSWORD.replace(' ', "+"));
    let response = h
        .router
        .handle(post(&format!("/{mount}/login"), &form, None))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), format!("/{mount}/orders"));
    session_cookie(&response).ok_or_else(|| anyhow::anyhow!("no session cookie"))
}

fn address() -> Address {
    Address {
        first_name: "Jonathan".into(),
        last_name: "Lapiquonne".into(),
        line1: "La agnès".into(),
        postal_code: "97290".into(),
        city: "Le Marin".into(),
        country_code: "MQ".into(),
        ..Address::default()
    }
}

async fn place_order(h: &Harness) -> anyhow::Result<String> {
    let id = timada_order::Command(&h.executor)
        .place_order(PlaceOrder {
            cart_id: "cart-1".into(),
            customer_id: "customer-1".into(),
            seller: Default::default(),
            lines: vec![OrderLine {
                product_id: "aoc-24g4xe".into(),
                name: "AOC 23.8\" LED - 24G4XE".into(),
                quantity: 1,
                unit_price: Money::eur(11_995),
                warranty_months: 60,
            }],
            delivery_address: address(),
            billing_address: address(),
            delivery: timada_order::DeliveryChoice {
                method_code: "chronopost-dom".into(),
                pickup_store_id: None,
            },
            payment_mode: timada_order::PaymentMode::Card,
            shipping_fee: Money::eur(2_395),
            handling_fee: Money::eur(0),
            promo_code: None,
            discount: None,
            order_number: Some("C2026-000042".into()),
        })
        .await?;
    order_history_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    Ok(id)
}

#[tokio::test]
async fn anonymous_requests_are_sent_to_login() -> anyhow::Result<()> {
    let h = harness("admin").await?;

    let response = h.router.handle(get("/admin/orders", None)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/admin/login?next=%2Fadmin%2Forders");

    let response = h.router.handle(get("/admin", None)).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), "/admin/orders");

    let response = h.router.handle(get("/admin/login", None)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await?;
    assert!(body.contains("Connexion"));
    assert!(body.contains("href=\"/dev.css\""));
    Ok(())
}

#[tokio::test]
async fn login_rejects_bad_credentials_and_opens_a_session() -> anyhow::Result<()> {
    let h = harness("admin").await?;

    let response = h
        .router
        .handle(post(
            "/admin/login",
            "email=admin%40timada.example&password=nope",
            None,
        ))
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(text(response).await?.contains("incorrect"));

    let cookie = sign_in(&h, "admin").await?;
    assert!(cookie.starts_with("__Host-timada_admin="));

    let response = h.router.handle(get("/admin/orders", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await?;
    assert!(body.contains("Commandes"));
    assert!(body.contains(EMAIL));

    let response = h
        .router
        .handle(post("/admin/logout", "", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admin_session")
        .fetch_one(&h.db)
        .await?;
    assert_eq!(count, 0);
    let response = h.router.handle(get("/admin/orders", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    Ok(())
}

#[tokio::test]
async fn orders_can_be_listed_viewed_and_cancelled() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let order_id = place_order(&h).await?;
    let cookie = sign_in(&h, "admin").await?;

    let body = text(h.router.handle(get("/admin/orders", Some(&cookie))).await).await?;
    assert!(body.contains(&order_id));
    assert!(body.contains("C2026-000042"), "{body}");
    assert!(body.contains("143,90"), "total should be formatted: {body}");
    let found = h
        .router
        .handle(get("/admin/orders?number=C2026-0000", Some(&cookie)))
        .await;
    assert!(text(found).await?.contains("C2026-000042"));
    let none = h
        .router
        .handle(get("/admin/orders?number=C1999", Some(&cookie)))
        .await;
    assert!(text(none).await?.contains("Aucune commande."));

    let detail = format!("/admin/orders/{order_id}");
    let body = text(h.router.handle(get(&detail, Some(&cookie))).await).await?;
    assert!(body.contains("Commande C2026-000042"), "{body}");
    assert!(body.contains("AOC 23.8"));
    assert!(body.contains("Annuler la commande"));

    let response = h
        .router
        .handle(post(
            &format!("{detail}/cancel"),
            "reason=client+request",
            Some(&cookie),
        ))
        .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&response), detail);
    let order = load_order_details(&h.executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.status, OrderStatus::Cancelled);
    assert_eq!(order.cancelled_reason.as_deref(), Some("client request"));

    let response = h
        .router
        .handle(get("/admin/orders/nope", Some(&cookie)))
        .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(text(response).await?.contains("Page introuvable"));
    Ok(())
}

#[tokio::test]
async fn unknown_admin_urls_render_the_branded_404() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let response = h.router.handle(get("/admin/nope/deeper", None)).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(text(response).await?.contains("Page introuvable"));
    Ok(())
}

#[tokio::test]
async fn the_mount_segment_is_configurable() -> anyhow::Result<()> {
    let h = harness("ops").await?;
    let cookie = sign_in(&h, "ops").await?;

    let response = h.router.handle(get("/ops/products", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = text(response).await?;
    assert!(body.contains("href=\"/ops/orders\""));
    assert!(body.contains("href=\"/ops/customers\""));
    assert!(body.contains("href=\"/ops/products/new\""));

    let response = h.router.handle(get("/admin/products", Some(&cookie))).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn promo_codes_and_vouchers_are_created_listed_and_ended() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;

    let empty = h
        .router
        .handle(get("/admin/promotions", Some(&cookie)))
        .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(text(empty).await?.contains("Aucun code."));

    // A bad optional number is a form error, not a 400.
    let bad = h
        .router
        .handle(post(
            "/admin/promotions/new-discount",
            "code=ete15&kind=percent&value=1500&max_redemptions=beaucoup&valid_days=",
            Some(&cookie),
        ))
        .await;
    assert_eq!(bad.status(), StatusCode::OK);
    assert!(text(bad).await?.contains("nombre entier attendu"));

    let created = h
        .router
        .handle(post(
            "/admin/promotions/new-discount",
            "code=ete15&kind=percent&value=1500&max_redemptions=100&valid_days=30",
            Some(&cookie),
        ))
        .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let discount_url = location(&created);
    assert_eq!(
        discount_url,
        format!(
            "/admin/promotions/{}",
            timada_promotion::discount_id("ETE15")
        )
    );
    let detail = text(h.router.handle(get(&discount_url, Some(&cookie))).await).await?;
    assert!(detail.contains("ETE15"));
    assert!(detail.contains("15 %"));
    assert!(detail.contains("0 / 100"));

    let duplicate = h
        .router
        .handle(post(
            "/admin/promotions/new-discount",
            "code=ETE15&kind=fixed&value=500&max_redemptions=&valid_days=",
            Some(&cookie),
        ))
        .await;
    assert_eq!(duplicate.status(), StatusCode::OK);
    assert!(text(duplicate).await?.contains("already exists"));

    let issued = h
        .router
        .handle(post(
            "/admin/promotions/new-voucher",
            "code=cadeau50&value_cents=5000&customer_id=&valid_days=",
            Some(&cookie),
        ))
        .await;
    assert_eq!(issued.status(), StatusCode::SEE_OTHER);
    let voucher_url = location(&issued);

    timada_promotion::code_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    let list = text(
        h.router
            .handle(get("/admin/promotions", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(list.contains("ETE15"));
    assert!(list.contains("CADEAU50"));
    let vouchers = text(
        h.router
            .handle(get("/admin/promotions?kind=voucher", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(!vouchers.contains("ETE15"));

    // Ending each kind of code.
    let deactivated = h
        .router
        .handle(post(
            &format!("{discount_url}/deactivate"),
            "",
            Some(&cookie),
        ))
        .await;
    assert_eq!(location(&deactivated), discount_url);
    let detail = text(h.router.handle(get(&discount_url, Some(&cookie))).await).await?;
    assert!(detail.contains("inactif"));
    assert!(!detail.contains("Désactiver le code"));

    let cancelled = h
        .router
        .handle(post(
            &format!("{voucher_url}/cancel"),
            "reason=erreur+de+saisie",
            Some(&cookie),
        ))
        .await;
    assert_eq!(location(&cancelled), voucher_url);
    let detail = text(h.router.handle(get(&voucher_url, Some(&cookie))).await).await?;
    assert!(detail.contains("erreur de saisie"));
    assert!(detail.contains("50,00 €"));

    let missing = h
        .router
        .handle(get("/admin/promotions/nope", Some(&cookie)))
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn stock_is_tracked_received_and_filtered() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;
    timada_catalog::Command(&h.executor)
        .create_product(timada_catalog::CreateProduct {
            sku: "aoc-24g4xe".into(),
            name: "AOC 23.8\" LED - 24G4XE".into(),
            brand: timada_catalog::Brand {
                name: "AOC".into(),
                slug: "aoc".into(),
            },
            category_path: vec!["Ecran PC".into()],
            short_description: "Ecran PC Full HD".into(),
            warranty_months: 60,
        })
        .await?;
    timada_catalog::product_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;

    let unknown = h
        .router
        .handle(post(
            "/admin/inventory/new",
            "sku=NOPE&store_id=&quantity=3",
            Some(&cookie),
        ))
        .await;
    assert_eq!(unknown.status(), StatusCode::OK);
    assert!(text(unknown).await?.contains("Aucun produit"));

    // Tracked in the warehouse with a first receipt of 3 units.
    let tracked = h
        .router
        .handle(post(
            "/admin/inventory/new",
            "sku=aoc-24g4xe&store_id=&quantity=3",
            Some(&cookie),
        ))
        .await;
    assert_eq!(tracked.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&tracked), "/admin/inventory");
    let again = h
        .router
        .handle(post(
            "/admin/inventory/new",
            "sku=AOC-24G4XE&store_id=&quantity=1",
            Some(&cookie),
        ))
        .await;
    assert!(text(again).await?.contains("déjà suivi"));

    timada_inventory::stock_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    let rows = timada_inventory::list_stock(&h.db, &timada_inventory::ListStock::default()).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].on_hand, rows[0].available), (3, 3));
    let list = text(
        h.router
            .handle(get("/admin/inventory", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(list.contains("AOC 23.8"));
    assert!(list.contains("Entrepôt"));

    // A receipt shows at once, before the list table catches up.
    let received = h
        .router
        .handle(post(
            "/admin/inventory/receive",
            &format!("stock_item_id={}&quantity=4", rows[0].stock_item_id),
            Some(&cookie),
        ))
        .await;
    assert_eq!(location(&received), "/admin/inventory");
    let list = text(
        h.router
            .handle(get("/admin/inventory", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(list.contains(">7<"), "3 + 4 units: {list}");

    // The low-stock filter reads the table, so it follows once synced.
    timada_inventory::stock_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    let low = text(
        h.router
            .handle(get("/admin/inventory?below=5", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(low.contains("Aucun article"));
    let low = text(
        h.router
            .handle(get("/admin/inventory?below=8", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(low.contains("AOC 23.8"));
    Ok(())
}

#[tokio::test]
async fn invoices_are_listed_and_payments_refunded() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;
    let order_id = place_order(&h).await?;

    // Stand in for the fulfillment saga: request, capture, mark paid.
    let payments = timada_payment::Command(&h.executor);
    let payment_id = payments
        .request_payment(timada_payment::RequestPayment {
            order_id: order_id.clone(),
            amount: Money::eur(14_390),
            method: timada_payment::PaymentMethod::Card,
        })
        .await?;
    payments
        .capture_payment(&payment_id, "psp-1".into())
        .await?;
    timada_order::Command(&h.executor)
        .mark_paid(&order_id, &payment_id)
        .await?;
    timada_invoice::invoice_from_orders_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    timada_invoice::invoice_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;

    let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    let number = format!("F{year}-000001");
    let list = text(h.router.handle(get("/admin/invoices", Some(&cookie))).await).await?;
    assert!(list.contains(&number), "{list}");
    let drafts = h
        .router
        .handle(get("/admin/invoices?status=draft", Some(&cookie)))
        .await;
    assert!(text(drafts).await?.contains("Aucune facture."));

    let invoice_id = timada_invoice::invoice_id(&order_id);
    let detail = h
        .router
        .handle(get(&format!("/admin/invoices/{invoice_id}"), Some(&cookie)))
        .await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = text(detail).await?;
    assert!(detail.contains(&format!("Facture {number}")), "{detail}");
    assert!(detail.contains("AOC 23.8"), "{detail}");
    let missing = h
        .router
        .handle(get("/admin/invoices/nope", Some(&cookie)))
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    // The order page links the invoice and offers the refund.
    let order_uri = format!("/admin/orders/{order_id}");
    let order_page = text(h.router.handle(get(&order_uri, Some(&cookie))).await).await?;
    assert!(order_page.contains(&number), "{order_page}");
    assert!(order_page.contains("Rembourser"), "{order_page}");

    let refunded = h
        .router
        .handle(post(
            &format!("{order_uri}/refund"),
            "amount_cents=2000&reason=Geste+commercial",
            Some(&cookie),
        ))
        .await;
    assert_eq!(refunded.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&refunded), order_uri);
    let payment = timada_payment::load_payment(&h.executor, &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(2_000));

    // More than what is left is refused with a message, nothing is written.
    let too_much = h
        .router
        .handle(post(
            &format!("{order_uri}/refund"),
            "amount_cents=99999&reason=Oups",
            Some(&cookie),
        ))
        .await;
    let back = location(&too_much);
    assert_eq!(back, format!("{order_uri}?refund_error=exceeds"));
    let order_page = text(h.router.handle(get(&back, Some(&cookie))).await).await?;
    assert!(order_page.contains("dépasse"), "{order_page}");
    assert!(order_page.contains("20,00 €"), "{order_page}");

    timada_payment::refund_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    let refunds = text(h.router.handle(get("/admin/refunds", Some(&cookie))).await).await?;
    assert!(refunds.contains("Geste commercial"), "{refunds}");
    assert!(refunds.contains("20,00 €"), "{refunds}");
    assert!(!refunds.contains("Oups"), "{refunds}");

    // The refund's credit note shows in the journal and under the invoice.
    timada_invoice::credit_notes_from_refunds_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    timada_invoice::credit_note_list_subscription()
        .data(h.db.clone())
        .run_once(&h.executor)
        .await?;
    let credit_note = format!("A{year}-000001");
    let refunds = text(h.router.handle(get("/admin/refunds", Some(&cookie))).await).await?;
    assert!(refunds.contains(&credit_note), "{refunds}");
    let detail = h
        .router
        .handle(get(&format!("/admin/invoices/{invoice_id}"), Some(&cookie)))
        .await;
    let detail = text(detail).await?;
    assert!(detail.contains(&credit_note), "{detail}");
    assert!(detail.contains("Net après avoirs"), "{detail}");
    assert!(detail.contains("123,90 €"), "{detail}");
    Ok(())
}

#[tokio::test]
async fn reviews_are_moderated_from_the_queue() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;
    let reviews = timada_review::Command(&h.executor);
    let submit = |customer: &str, body: &str| timada_review::SubmitReview {
        product_id: "aoc-24g4xe".into(),
        customer_id: customer.into(),
        order_id: Some("order-1".into()),
        rating: 4,
        title: "Bon écran".into(),
        body: body.into(),
    };
    let kept = reviews
        .submit_review(submit("customer-1", "Rien à redire."))
        .await?;
    let refused = reviews
        .submit_review(submit("customer-2", "Texte hors sujet."))
        .await?;
    let sync = || async {
        timada_review::review_list_subscription()
            .data(h.db.clone())
            .run_once(&h.executor)
            .await
    };
    sync().await?;

    // Both wait in the queue, which is the default view.
    let queue = text(h.router.handle(get("/admin/reviews", Some(&cookie))).await).await?;
    assert!(queue.contains("Rien à redire."), "{queue}");
    assert!(queue.contains("Texte hors sujet."), "{queue}");
    assert!(queue.contains("Achat vérifié"), "{queue}");

    let published = h
        .router
        .handle(post(
            "/admin/reviews/publish",
            &format!("review_id={kept}"),
            Some(&cookie),
        ))
        .await;
    assert_eq!(published.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&published), "/admin/reviews");
    let rejected = h
        .router
        .handle(post(
            "/admin/reviews/reject",
            &format!("review_id={refused}&reason=Hors+sujet"),
            Some(&cookie),
        ))
        .await;
    assert_eq!(rejected.status(), StatusCode::SEE_OTHER);
    // Moderating twice (a double click, another operator) changes nothing.
    let again = h
        .router
        .handle(post(
            "/admin/reviews/reject",
            &format!("review_id={kept}&reason=Trop+tard"),
            Some(&cookie),
        ))
        .await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    sync().await?;

    let view = timada_review::load_review_details(&h.executor, &kept)
        .await?
        .ok_or_else(|| anyhow::anyhow!("review missing"))?;
    assert_eq!(view.status, timada_review::ReviewStatus::Published);

    let queue = text(h.router.handle(get("/admin/reviews", Some(&cookie))).await).await?;
    assert!(queue.contains("Aucun avis dans cette file."), "{queue}");
    let refused_list = h
        .router
        .handle(get("/admin/reviews?status=rejected", Some(&cookie)))
        .await;
    let refused_list = text(refused_list).await?;
    assert!(refused_list.contains("Hors sujet"), "{refused_list}");
    assert!(!refused_list.contains("Rien à redire."), "{refused_list}");
    let all = text(
        h.router
            .handle(get("/admin/reviews?status=all", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(all.contains("Rien à redire."), "{all}");
    assert!(all.contains("Publié"), "{all}");
    Ok(())
}

#[tokio::test]
async fn questions_are_answered_from_the_queue() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;
    let question = timada_review::Command(&h.executor)
        .ask_question(timada_review::AskQuestion {
            product_id: "aoc-24g4xe".into(),
            customer_id: "customer-1".into(),
            body: "Compatible G-SYNC ?".into(),
        })
        .await?;
    let sync = || async {
        timada_review::question_list_subscription()
            .data(h.db.clone())
            .run_once(&h.executor)
            .await
    };
    sync().await?;

    let queue = text(
        h.router
            .handle(get("/admin/questions", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(queue.contains("Compatible G-SYNC ?"), "{queue}");
    assert!(queue.contains("Sans réponse"), "{queue}");

    // An empty answer is refused with a message, nothing is written.
    let empty = h
        .router
        .handle(post(
            "/admin/questions/answer",
            &format!("question_id={question}&body=+"),
            Some(&cookie),
        ))
        .await;
    assert_eq!(location(&empty), "/admin/questions?error=empty");
    let warned = text(h.router.handle(get(&location(&empty), Some(&cookie))).await).await?;
    assert!(warned.contains("Écrivez une réponse"), "{warned}");

    let answered = h
        .router
        .handle(post(
            "/admin/questions/answer",
            &format!("question_id={question}&body=Oui%2C+G-SYNC+Compatible."),
            Some(&cookie),
        ))
        .await;
    assert_eq!(answered.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&answered), "/admin/questions");
    sync().await?;

    // Answered: out of the default queue, listed with its answer elsewhere.
    let queue = text(
        h.router
            .handle(get("/admin/questions", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(
        queue.contains("Aucune question dans cette file."),
        "{queue}"
    );
    let done = h
        .router
        .handle(get("/admin/questions?status=answered", Some(&cookie)))
        .await;
    let done = text(done).await?;
    assert!(done.contains("Oui, G-SYNC Compatible."), "{done}");
    assert!(done.contains("Boutique"), "{done}");
    Ok(())
}

#[tokio::test]
async fn the_outbox_is_listed_and_failed_emails_can_be_retried() -> anyhow::Result<()> {
    struct Down;
    impl timada_mailer::Transport for Down {
        fn send<'a>(&'a self, _email: &'a timada_mailer::Email) -> timada_mailer::SendFuture<'a> {
            Box::pin(async { Err(timada_mailer::MailError::Transport("relay down".into())) })
        }
    }

    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;
    let email = timada_mailer::Email {
        from: "Timada <no-reply@timada.example>".into(),
        to: "ada@example.com".into(),
        subject: "Confirmation de votre commande C2026-000042".into(),
        body: "Bonjour Ada,\n\nMerci pour votre commande.".into(),
    };
    timada_mailer::enqueue(&h.db, "m-1", "order-confirmation", &email).await?;

    let list = text(h.router.handle(get("/admin/emails", Some(&cookie))).await).await?;
    assert!(list.contains("ada@example.com"), "{list}");
    assert!(list.contains("En attente"), "{list}");

    // The relay refuses it until the mailer gives up.
    for _ in 0..timada_mailer::MAX_ATTEMPTS {
        timada_mailer::deliver_pending(&h.db, &Down).await?;
    }
    let failed = h
        .router
        .handle(get("/admin/emails?status=failed", Some(&cookie)))
        .await;
    assert!(
        text(failed)
            .await?
            .contains("Confirmation de votre commande")
    );
    let detail = text(
        h.router
            .handle(get("/admin/emails/m-1", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(detail.contains("Merci pour votre commande."), "{detail}");
    assert!(detail.contains("relay down"), "{detail}");
    assert!(detail.contains("Réessayer"), "{detail}");

    // An operator retries it; the next pass delivers.
    let retried = h
        .router
        .handle(post("/admin/emails/m-1/retry", "", Some(&cookie)))
        .await;
    assert_eq!(location(&retried), "/admin/emails/m-1");
    let delivered = timada_mailer::deliver_pending(&h.db, &timada_mailer::LogTransport).await?;
    assert_eq!(delivered.sent, 1);
    let detail = text(
        h.router
            .handle(get("/admin/emails/m-1", Some(&cookie)))
            .await,
    )
    .await?;
    assert!(detail.contains("Envoyé"), "{detail}");
    assert!(!detail.contains("Réessayer"), "{detail}");
    let missing = h
        .router
        .handle(get("/admin/emails/nope", Some(&cookie)))
        .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn returns_are_reviewed_and_received_from_the_queue() -> anyhow::Result<()> {
    let h = harness("admin").await?;
    let cookie = sign_in(&h, "admin").await?;
    let order_id = place_order(&h).await?;

    // Paid and shipped, then the customer asks to send the monitor back.
    let payments = timada_payment::Command(&h.executor);
    let payment_id = payments
        .request_payment(timada_payment::RequestPayment {
            order_id: order_id.clone(),
            amount: Money::eur(14_390),
            method: timada_payment::PaymentMethod::Card,
        })
        .await?;
    payments
        .capture_payment(&payment_id, "psp-1".into())
        .await?;
    let orders = timada_order::Command(&h.executor);
    orders.mark_paid(&order_id, &payment_id).await?;
    orders
        .mark_shipped(&order_id, "shipment-1", "Chronopost".into(), "XY123".into())
        .await?;
    let returns = timada_returns::Command {
        executor: &h.executor,
        db: h.db.clone(),
        policy: timada_returns::ReturnPolicy::default(),
    };
    let return_id = returns
        .request_return(timada_returns::RequestReturn {
            order_id: order_id.clone(),
            customer_id: "customer-1".into(),
            lines: vec![timada_returns::RequestedLine {
                product_id: "aoc-24g4xe".into(),
                quantity: 1,
            }],
            reason: "Pixel mort".into(),
        })
        .await?;
    let sync = || async {
        for _ in 0..2 {
            timada_returns::return_processing_subscription()
                .data(h.db.clone())
                .run_once(&h.executor)
                .await?;
            timada_returns::return_list_subscription()
                .data(h.db.clone())
                .run_once(&h.executor)
                .await?;
        }
        anyhow::Ok(())
    };
    sync().await?;

    // The queue opens on what is to be reviewed.
    let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    let rma = format!("R{year}-000001");
    let queue = text(h.router.handle(get("/admin/returns", Some(&cookie))).await).await?;
    assert!(queue.contains(&rma), "{queue}");
    assert!(queue.contains("Pixel mort"), "{queue}");
    assert!(queue.contains("C2026-000042"), "{queue}");

    let detail_uri = format!("/admin/returns/{return_id}");
    let detail = text(h.router.handle(get(&detail_uri, Some(&cookie))).await).await?;
    assert!(detail.contains("Accepter le retour"), "{detail}");
    assert!(!detail.contains("Réceptionner le colis"), "{detail}");

    let approved = h
        .router
        .handle(post(&format!("{detail_uri}/approve"), "", Some(&cookie)))
        .await;
    assert_eq!(location(&approved), detail_uri);
    let detail = text(h.router.handle(get(&detail_uri, Some(&cookie))).await).await?;
    assert!(detail.contains("Réceptionner le colis"), "{detail}");

    // More than was asked for is refused with a message.
    let too_many = h
        .router
        .handle(post(
            &format!("{detail_uri}/receive"),
            "product_0=aoc-24g4xe&accepted_0=3&restock_0=on&refund_method=original",
            Some(&cookie),
        ))
        .await;
    assert_eq!(location(&too_many), format!("{detail_uri}?error=accepted"));
    let warned = text(
        h.router
            .handle(get(&location(&too_many), Some(&cookie)))
            .await,
    )
    .await?;
    assert!(warned.contains("articles que demandé"), "{warned}");

    // Taken back, damaged (box unchecked), refunded as store credit.
    let received = h
        .router
        .handle(post(
            &format!("{detail_uri}/receive"),
            "product_0=aoc-24g4xe&accepted_0=1&refund_method=credit",
            Some(&cookie),
        ))
        .await;
    assert_eq!(location(&received), detail_uri);
    sync().await?;

    let detail = text(h.router.handle(get(&detail_uri, Some(&cookie))).await).await?;
    assert!(detail.contains("Traité"), "{detail}");
    assert!(detail.contains("non remis en stock"), "{detail}");
    assert!(detail.contains(&format!("AVOIR-{rma}")), "{detail}");
    assert!(detail.contains("119,95 €"), "{detail}");
    let payment = timada_payment::load_payment(&h.executor, &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, Money::eur(0));

    // Out of the default queue, listed under its status, linked from the order.
    let queue = text(h.router.handle(get("/admin/returns", Some(&cookie))).await).await?;
    assert!(queue.contains("Aucun retour dans cette file."), "{queue}");
    let done = h
        .router
        .handle(get("/admin/returns?status=completed", Some(&cookie)))
        .await;
    assert!(text(done).await?.contains(&rma));
    let order_page = h
        .router
        .handle(get(&format!("/admin/orders/{order_id}"), Some(&cookie)))
        .await;
    assert!(text(order_page).await?.contains(&rma));
    Ok(())
}
