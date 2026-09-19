//! Socket-free tests through `Router::handle`: auth, mounting under a custom
//! segment, the orders section, branded 404s.

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
    assert!(body.contains("143,90"), "total should be formatted: {body}");

    let detail = format!("/admin/orders/{order_id}");
    let body = text(h.router.handle(get(&detail, Some(&cookie))).await).await?;
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
