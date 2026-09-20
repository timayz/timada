//! Socket-free tests through `Router::handle`: a guest fills a cart, signs
//! up, adds an address, checks out, and follows the order the process
//! managers place.

use std::collections::BTreeMap;

use timada_admin::Stylesheet;
use timada_catalog::{ListProducts, list_products};
use timada_promotion::{CreateDiscount, DiscountKind};
use topcoat::{
    asset::{AssetCatalog, AssetConfig},
    router::{Body, Router, StatusCode, request::Request, response::Response, to_bytes},
};

use crate::{Store, db, seed};

/// A browser: the router plus the cookies it was handed.
struct Browser<'a> {
    router: &'a Router,
    cookies: BTreeMap<String, String>,
}

impl<'a> Browser<'a> {
    fn new(router: &'a Router) -> Self {
        Self {
            router,
            cookies: BTreeMap::new(),
        }
    }

    fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    async fn send(&mut self, builder: http::request::Builder, body: Body) -> Response {
        let request: Request = builder
            .header("cookie", self.cookie_header())
            .body(body)
            .unwrap_or_default();
        let response = self.router.handle(request).await;
        for header in response.headers().get_all("set-cookie") {
            let Ok(header) = header.to_str() else {
                continue;
            };
            let Some((name, value)) = header.split(';').next().and_then(|p| p.split_once('='))
            else {
                continue;
            };
            if value.is_empty() || header.contains("Max-Age=0") {
                self.cookies.remove(name);
            } else {
                self.cookies.insert(name.to_owned(), value.to_owned());
            }
        }
        response
    }

    async fn get(&mut self, uri: &str) -> Response {
        self.send(http::Request::builder().uri(uri), Body::empty())
            .await
    }

    async fn post(&mut self, uri: &str, form: &str) -> Response {
        let builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("sec-fetch-site", "same-origin");
        self.send(builder, Body::from(form.to_owned())).await
    }
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

/// A seeded shop: one product (5 in stock, 119,95 €), one shopper, one order.
async fn shop() -> anyhow::Result<(Router, Store, String)> {
    shop_with(std::sync::Arc::new(timada_payment::ManualProvider)).await
}

/// [`shop`], taking its payments through `provider`.
async fn shop_with(
    provider: std::sync::Arc<dyn timada_payment::PaymentProvider>,
) -> anyhow::Result<(Router, Store, String)> {
    shop_checking(provider, timada_tax::FakeValidator::default()).await
}

/// [`shop_with`], checking VAT numbers against `registry` — a handle the test
/// keeps to script its answers.
async fn shop_checking(
    provider: std::sync::Arc<dyn timada_payment::PaymentProvider>,
    registry: timada_tax::FakeValidator,
) -> anyhow::Result<(Router, Store, String)> {
    let (executor, pool) = timada_core::testing::memory_executor(db::migrations()).await?;
    let store = Store {
        executor,
        archive: timada_invoice::InvoiceArchive::new(timada_invoice::SqliteArchiveStore::new(
            pool.clone(),
        )),
        db: pool,
        provider,
        #[cfg(feature = "stripe")]
        stripe: None,
        vat_validator: std::sync::Arc::new(registry),
    };
    seed::run(&store).await?;
    db::run_subscriptions_once(&store).await?;
    let product = list_products(&store.db, &ListProducts::default())
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no seeded product"))?;
    let router = crate::router(
        store.clone(),
        AssetConfig::hosted_at("/assets", AssetCatalog::default()),
        Stylesheet::Url("/dev.css".into()),
    );
    Ok((router, store, product.id))
}

const REGISTER: &str = "civility=mrs&first_name=Ada&last_name=Lovelace&email=ada%40example.com&password=analytical-engine&next=%2Fcheckout";
const ADDRESS: &str = "civility=mrs&first_name=Ada&last_name=Lovelace&line1=12+rue+des+Machines&line2=&postal_code=31000&city=Toulouse&country_code=fr&phone=&mobile=&next=%2Fcheckout";

#[tokio::test]
async fn guest_cart_to_placed_order() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);

    // The product page offers the cart; adding issues the cart cookie.
    let page = text(browser.get(&format!("/p/{product_id}")).await).await?;
    assert!(page.contains("Ajouter au panier"));
    let added = browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    assert_eq!(added.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&added), "/cart");
    assert!(browser.cookies.contains_key("__Host-timada_cart"));

    // Adding the same product again tops the line up.
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("AOC 23.8"));
    assert!(cart.contains("239,90 €"), "two units: {cart}");
    assert!(cart.contains("Panier (2)"));

    // Stock is 5, one of which the seeded order reserved.
    let too_many = browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=9"))
        .await;
    assert_eq!(too_many.status(), StatusCode::OK);
    assert!(text(too_many).await?.contains("Seulement 4 exemplaire"));

    // Checkout needs an account and comes back after sign-up.
    let gate = browser.get("/checkout").await;
    assert_eq!(gate.status(), StatusCode::SEE_OTHER);
    assert!(location(&gate).starts_with("/login?next="));
    let registered = browser.post("/register", REGISTER).await;
    assert_eq!(registered.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&registered), "/checkout");
    assert!(browser.cookies.contains_key("__Host-timada_shop"));

    // No address yet: the page asks for one, and the form returns to checkout.
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("Ajoutez une adresse de livraison"));
    let address = browser.post("/account/addresses/new", ADDRESS).await;
    assert_eq!(location(&address), "/checkout");
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("12 rue des Machines"));
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();

    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    assert_eq!(placed.status(), StatusCode::SEE_OTHER);
    let confirmation = location(&placed);
    assert!(confirmation.starts_with("/checkout/pay/"));
    assert!(!browser.cookies.contains_key("__Host-timada_cart"));

    // The order appears once the process managers have run.
    let pending = text(browser.get(&confirmation).await).await?;
    assert!(pending.contains("en cours d'enregistrement"));
    assert!(pending.contains("http-equiv=\"refresh\""));
    db::run_subscriptions_once(&store).await?;
    let done = text(browser.get(&confirmation).await).await?;
    assert!(done.contains("votre commande est enregistrée"));
    assert!(done.contains("245,80 €"), "2 × 119,95 + 5,90: {done}");
    // The shopper is given a readable number, not the internal id: the
    // second of the sequence, the seeded order took the first.
    let year = timada_core::time::year_of(timada_core::time::now_unix_secs()?);
    let number = format!("C{year}-000002");
    assert!(done.contains(&number), "{done}");

    let order_id = confirmation
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let history = text(browser.get("/account/orders").await).await?;
    assert!(history.contains(&order_id));
    assert!(history.contains(&number), "{history}");
    let detail = browser.get(&format!("/account/orders/{order_id}")).await;
    assert_eq!(detail.status(), StatusCode::OK);
    let detail = text(detail).await?;
    assert!(detail.contains(&format!("Commande {number}")), "{detail}");
    // 245,80 TTC at 20 %: 204,83 HT, 40,97 of VAT.
    assert!(detail.contains("dont TVA 20 % sur 204,83 €"), "{detail}");
    assert!(detail.contains("40,97 €"), "{detail}");

    // The confirmation e-mail waits in the outbox for the delivery worker.
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    let confirmation = outbox
        .iter()
        .find(|m| m.recipient == "ada@example.com" && m.kind == "order-confirmation")
        .ok_or_else(|| anyhow::anyhow!("no confirmation to the shopper: {outbox:?}"))?;
    // Signing up had already written the welcome.
    assert!(
        outbox
            .iter()
            .any(|m| m.recipient == "ada@example.com" && m.kind == "welcome"),
        "{outbox:?}"
    );
    assert!(
        confirmation.subject.contains(&number),
        "{}",
        confirmation.subject
    );
    assert!(
        confirmation.body.contains("245,80 €"),
        "{}",
        confirmation.body
    );
    assert!(detail.contains("12 rue des Machines"));
    assert!(detail.contains("Carte bancaire"));

    // Another shopper cannot read it.
    let mut other = Browser::new(&router);
    let login = other
        .post(
            "/login",
            &format!(
                "email={}&password={}",
                seed::SHOPPER_EMAIL.replace('@', "%40"),
                seed::SHOPPER_PASSWORD
            ),
        )
        .await;
    assert_eq!(location(&login), "/account");
    let foreign = other.get(&format!("/account/orders/{order_id}")).await;
    assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    let own = text(other.get("/account/orders").await).await?;
    assert!(!own.contains(&order_id));
    Ok(())
}

#[tokio::test]
async fn accounts_are_unique_and_passwords_checked() -> anyhow::Result<()> {
    let (router, _store, _) = shop().await?;
    let mut browser = Browser::new(&router);

    assert_eq!(
        browser.get("/account").await.status(),
        StatusCode::SEE_OTHER
    );
    let weak = browser
        .post(
            "/register",
            "civility=mr&first_name=A&last_name=B&email=a%40example.com&password=short",
        )
        .await;
    assert!(text(weak).await?.contains("au moins 8 caractères"));

    let taken = browser
        .post(
            "/register",
            "civility=mr&first_name=A&last_name=B&email=JONATHAN%40example.com&password=long-enough-password",
        )
        .await;
    assert_eq!(taken.status(), StatusCode::OK);
    assert!(text(taken).await?.contains("existe déjà"));

    let wrong = browser
        .post("/login", "email=jonathan%40example.com&password=nope-nope")
        .await;
    assert_eq!(wrong.status(), StatusCode::OK);
    assert!(text(wrong).await?.contains("incorrect"));
    assert!(!browser.cookies.contains_key("__Host-timada_shop"));

    // Open redirects are refused; logout closes the session.
    let login = browser
        .post(
            "/login",
            "email=jonathan%40example.com&password=demo1234&next=%2F%2Fevil.example",
        )
        .await;
    assert_eq!(location(&login), "/account");
    let overview = text(browser.get("/account").await).await?;
    assert!(overview.contains("Jonathan"));
    let addresses = text(browser.get("/account/addresses").await).await?;
    assert!(addresses.contains("121, Avenue Tolosane"));
    assert!(addresses.contains("Adresse préférée"));

    browser.post("/logout", "").await;
    assert_eq!(
        browser.get("/account").await.status(),
        StatusCode::SEE_OTHER
    );
    Ok(())
}

#[tokio::test]
async fn only_honoured_promo_codes_are_recorded() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;

    let unknown = browser.post("/cart/promo", "code=NOPE").await;
    assert_eq!(unknown.status(), StatusCode::OK);
    assert!(text(unknown).await?.contains("inconnu"));

    timada_promotion::Command {
        executor: &store.executor,
        db: store.db.clone(),
    }
    .create_discount(CreateDiscount {
        code: "rentree10".into(),
        kind: DiscountKind::Percent { bp: 1_000 },
        max_redemptions: None,
        valid_until: Some(1),
    })
    .await?;
    let expired = browser.post("/cart/promo", "code=rentree10").await;
    assert!(text(expired).await?.contains("expiré"));

    timada_promotion::Command {
        executor: &store.executor,
        db: store.db.clone(),
    }
    .create_discount(CreateDiscount {
        code: "bienvenue".into(),
        kind: DiscountKind::Percent { bp: 500 },
        max_redemptions: None,
        valid_until: None,
    })
    .await?;
    let applied = browser.post("/cart/promo", "code=bienvenue").await;
    assert_eq!(applied.status(), StatusCode::SEE_OTHER);
    assert!(
        text(browser.get("/cart").await)
            .await?
            .contains("BIENVENUE")
    );

    let removed = browser.post("/cart/promo/remove", "").await;
    assert_eq!(removed.status(), StatusCode::SEE_OTHER);
    let cart = text(browser.get("/cart").await).await?;
    assert!(!cart.contains("BIENVENUE"), "{cart}");
    Ok(())
}

#[tokio::test]
async fn promo_code_lowers_the_order_total() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    let applied = browser
        .post("/cart/promo", &format!("code={}", seed::PROMO_CODE))
        .await;
    assert_eq!(applied.status(), StatusCode::SEE_OTHER);

    // 10 % off 119,95 €, shown before anything is ordered.
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("Code promo BIENVENUE10"), "{cart}");
    assert!(cart.contains("12,00 €"), "{cart}");
    assert!(cart.contains("107,95 €"), "{cart}");

    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("107,95 €"), "{checkout}");
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();
    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    let confirmation = location(&placed);
    db::run_subscriptions_once(&store).await?;

    // 107,95 + 5,90 of shipping: the order, its history row and its detail.
    let done = text(browser.get(&confirmation).await).await?;
    assert!(done.contains("113,85 €"), "{done}");
    let order_id = confirmation
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let history = text(browser.get("/account/orders").await).await?;
    assert!(history.contains("113,85 €"), "{history}");
    let detail = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(detail.contains("Remise (BIENVENUE10)"), "{detail}");
    assert!(detail.contains("113,85 €"), "{detail}");

    let invoice =
        timada_invoice::load_invoice(&store.executor, timada_invoice::invoice_id(&order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("invoice not drafted"))?;
    assert_eq!(invoice.total, timada_core::Money::eur(11_385));

    // Having ordered the product makes the shopper's review a verified purchase.
    browser
        .post(
            &format!("/p/{product_id}/reviews"),
            "rating=5&title=&body=Conforme",
        )
        .await;
    db::run_subscriptions_once(&store).await?;
    let reviews =
        timada_review::list_reviews(&store.db, &timada_review::ListReviews::default()).await?;
    assert_eq!(reviews.len(), 1);
    assert!(reviews[0].verified_purchase);
    Ok(())
}

#[tokio::test]
async fn cancelling_a_paid_order_refunds_it_with_a_credit_note() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    let checkout = text(browser.get("/checkout").await).await?;
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();
    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;

    // Paid, invoiced, then cancelled by an operator.
    let payment_id = timada_payment::payment_id(&order_id);
    timada_payment::Command(&store.executor)
        .capture_payment(&payment_id, "psp-1".into())
        .await?;
    db::run_subscriptions_once(&store).await?;
    timada_order::Command(&store.executor)
        .cancel_order(&order_id, "rupture fournisseur")
        .await?;
    db::run_subscriptions_once(&store).await?;

    let payment = timada_payment::load_payment(&store.executor, &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.status, timada_payment::PaymentStatus::Refunded);

    // The invoice stays issued; one credit note gives the whole total back.
    let invoice_id = timada_invoice::invoice_id(&order_id);
    let invoice = timada_invoice::load_invoice(&store.executor, &invoice_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("invoice missing"))?;
    assert_eq!(invoice.status, timada_invoice::InvoiceStatus::Issued);
    let notes = timada_invoice::credit_notes_of_invoice(&store.db, &invoice_id).await?;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].amount_minor, invoice.total.minor);
    assert!(notes[0].reason.contains("rupture fournisseur"));

    // The parcel that was waiting for the carrier will not leave.
    let shipment =
        timada_shipping::load_shipment(&store.executor, timada_shipping::shipment_id(&order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("shipment missing"))?;
    assert_eq!(shipment.status, timada_shipping::ShipmentStatus::Cancelled);

    // The shopper sees the refund and its credit note on the order page.
    let detail = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(detail.contains("Remboursé"), "{detail}");
    assert!(detail.contains(&notes[0].credit_note_number), "{detail}");
    assert!(detail.contains("rupture fournisseur"), "{detail}");

    // The credit note is a document of its own: the page links its PDF, which
    // is the file the archive filed when the note was issued.
    let note_uri = format!(
        "/account/orders/{order_id}/credit-notes/{}",
        notes[0].credit_note_id
    );
    assert!(detail.contains(&note_uri), "{detail}");
    let pdf = browser.get(&note_uri).await;
    assert_eq!(pdf.status(), StatusCode::OK);
    let disposition = pdf
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert_eq!(
        disposition,
        format!(
            "attachment; filename=\"avoir-{}.pdf\"",
            notes[0].credit_note_number
        )
    );
    let downloaded = to_bytes(pdf.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    assert!(downloaded.starts_with(b"%PDF-"));
    let (entry, archived) = timada_invoice::read_archived(
        &store.db,
        store.archive.0.as_ref(),
        &notes[0].credit_note_id,
    )
    .await?
    .ok_or_else(|| anyhow::anyhow!("credit note not archived"))?;
    assert_eq!(entry.kind, "credit_note");
    assert_eq!(downloaded.as_ref(), archived.as_slice());

    // It went to the shopper's mailbox as well, next to the refund e-mail.
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    let kinds: Vec<&str> = outbox.iter().map(|m| m.kind.as_str()).collect();
    assert!(kinds.contains(&"refund"), "{kinds:?}");
    let mailed = outbox
        .iter()
        .find(|m| m.kind == "credit-note-issued")
        .ok_or_else(|| anyhow::anyhow!("credit note not e-mailed: {kinds:?}"))?;
    assert_eq!(
        mailed.subject,
        format!("Votre avoir {}", notes[0].credit_note_number)
    );

    // Nobody else's, and not under another order.
    let mut other = Browser::new(&router);
    other
        .post(
            "/login",
            &format!(
                "email={}&password={}",
                seed::SHOPPER_EMAIL.replace('@', "%40"),
                seed::SHOPPER_PASSWORD
            ),
        )
        .await;
    assert_eq!(other.get(&note_uri).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        browser
            .get(&format!(
                "/account/orders/{order_id}/credit-notes/{invoice_id}"
            ))
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    Ok(())
}

#[tokio::test]
async fn reviews_wait_for_moderation_before_showing_on_the_product_page() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    let product = format!("/p/{product_id}");

    // Guests are invited to sign in; posting sends them to the login page.
    let page = text(browser.get(&product).await).await?;
    assert!(page.contains("Aucun avis pour le moment."), "{page}");
    assert!(page.contains("Connectez-vous"), "{page}");
    let guest = browser
        .post(
            &format!("{product}/reviews"),
            "rating=5&title=Top&body=Super",
        )
        .await;
    assert!(
        location(&guest).starts_with("/login"),
        "{}",
        location(&guest)
    );

    browser.post("/register", REGISTER).await;
    let empty = browser
        .post(&format!("{product}/reviews"), "rating=5&title=Top&body=+")
        .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(text(empty).await?.contains("Écrivez votre avis"));

    let sent = browser
        .post(
            &format!("{product}/reviews"),
            "rating=4&title=Tr%C3%A8s+bon+%C3%A9cran&body=Fluide+et+lumineux.",
        )
        .await;
    assert_eq!(location(&sent), format!("{product}#avis"));
    db::run_subscriptions_once(&store).await?;

    // Pending: the author is told so, nobody sees the text yet.
    let page = text(browser.get(&product).await).await?;
    assert!(page.contains("une fois validé"), "{page}");
    assert!(!page.contains("Fluide et lumineux."), "{page}");
    let twice = browser
        .post(
            &format!("{product}/reviews"),
            "rating=1&title=Bis&body=Encore",
        )
        .await;
    assert!(text(twice).await?.contains("déjà donné votre avis"));

    // Published by an operator: it shows, with the rating summary.
    let rows =
        timada_review::list_reviews(&store.db, &timada_review::ListReviews::default()).await?;
    assert_eq!(rows.len(), 1);
    assert!(!rows[0].verified_purchase);
    timada_review::Command(&store.executor)
        .publish_review(&rows[0].review_id)
        .await?;
    db::run_subscriptions_once(&store).await?;
    let page = text(Browser::new(&router).get(&product).await).await?;
    assert!(page.contains("Fluide et lumineux."), "{page}");
    assert!(page.contains("Ada L."), "{page}");
    assert!(page.contains("4,0 / 5 — 1 avis"), "{page}");
    Ok(())
}

#[tokio::test]
async fn questions_and_community_answers_go_through_moderation() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    let product = format!("/p/{product_id}");

    let page = text(browser.get(&product).await).await?;
    assert!(page.contains("Aucune question pour le moment."), "{page}");
    assert!(page.contains("pour poser une question"), "{page}");
    let guest = browser
        .post(
            &format!("{product}/questions"),
            "body=Compatible+G-SYNC+%3F",
        )
        .await;
    assert!(
        location(&guest).starts_with("/login"),
        "{}",
        location(&guest)
    );

    browser.post("/register", REGISTER).await;
    let empty = browser
        .post(&format!("{product}/questions"), "body=+")
        .await;
    assert_eq!(empty.status(), StatusCode::OK);
    assert!(text(empty).await?.contains("Écrivez votre question"));
    let asked = browser
        .post(
            &format!("{product}/questions"),
            "body=Compatible+G-SYNC+%3F",
        )
        .await;
    assert_eq!(location(&asked), format!("{product}#questions"));
    db::run_subscriptions_once(&store).await?;

    // Awaiting moderation: the asker sees it waiting, nobody else sees it.
    let own = text(browser.get(&product).await).await?;
    assert!(own.contains("en attente de validation"), "{own}");
    assert!(own.contains("Compatible G-SYNC ?"), "{own}");
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(!public.contains("Compatible G-SYNC ?"), "{public}");

    // Published by an operator: public, without an answer yet, and open to
    // the answers of signed-in shoppers.
    let reviews = timada_review::Command(&store.executor);
    let rows =
        timada_review::list_questions(&store.db, timada_review::QuestionFilter::All, 50, 0).await?;
    assert_eq!(rows.len(), 1);
    let question_id = rows[0].question_id.clone();
    reviews.publish_question(&question_id).await?;
    db::run_subscriptions_once(&store).await?;
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(public.contains("Compatible G-SYNC ?"), "{public}");
    assert!(public.contains("Pas encore de réponse."), "{public}");
    assert!(!public.contains("Votre réponse"), "{public}");

    // Another shopper answers: told it awaits moderation, not yet public.
    let mut helper = Browser::new(&router);
    helper
        .post(
            "/login",
            &format!(
                "email={}&password={}",
                seed::SHOPPER_EMAIL.replace('@', "%40"),
                seed::SHOPPER_PASSWORD
            ),
        )
        .await;
    let answers_uri = format!("{product}/questions/{question_id}/answers");
    let blank = helper.post(&answers_uri, "body=+").await;
    assert!(text(blank).await?.contains("Écrivez votre réponse"));
    let answered = helper
        .post(&answers_uri, "body=Oui%2C+valid%C3%A9+sur+RTX+4070.")
        .await;
    assert_eq!(location(&answered), format!("{product}#questions"));
    let twice = helper.post(&answers_uri, "body=Encore").await;
    assert!(text(twice).await?.contains("déjà répondu"));
    db::run_subscriptions_once(&store).await?;
    let theirs = text(helper.get(&product).await).await?;
    assert!(theirs.contains("sera visible une fois validée"), "{theirs}");
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(!public.contains("validé sur RTX 4070"), "{public}");

    // Through moderation: everyone reads it, and the asker is written to.
    let pending =
        timada_review::answers_of_questions(&store.db, std::slice::from_ref(&question_id), false)
            .await?;
    reviews
        .publish_answer(&question_id, &pending[0].answer_id)
        .await?;
    db::run_subscriptions_once(&store).await?;
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(public.contains("Oui, validé sur RTX 4070."), "{public}");
    assert!(public.contains("un client</strong>"), "{public}");
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    let told = outbox
        .iter()
        .find(|m| m.kind == "question-answered" && m.recipient == "ada@example.com")
        .ok_or_else(|| anyhow::anyhow!("the asker was not told: {outbox:?}"))?;
    assert!(told.body.contains("validé sur RTX 4070"), "{}", told.body);

    // The shop's own answer is public as written.
    reviews
        .answer_question(
            &question_id,
            timada_review::AnswerAuthor::Staff,
            "Oui, G-SYNC Compatible.".into(),
        )
        .await?;
    db::run_subscriptions_once(&store).await?;
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(public.contains("Réponse de la boutique"), "{public}");
    Ok(())
}
#[tokio::test]
async fn shoppers_are_told_when_a_product_is_back_in_stock() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    let product = format!("/p/{product_id}");
    let inventory = timada_inventory::Command(&store.executor);
    let stock_item =
        timada_inventory::stock_item_id(&product_id, &timada_inventory::StockLocation::Warehouse);

    // In stock: no alert to ask for, and asking anyway registers nothing.
    let page = text(browser.get(&product).await).await?;
    assert!(!page.contains("retour en stock"), "{page}");
    browser.post("/register", REGISTER).await;
    browser.post(&format!("{product}/alert"), "").await;
    db::run_subscriptions_once(&store).await?;
    let none = text(browser.get("/account/alerts").await).await?;
    assert!(none.contains("Aucune alerte."), "{none}");

    // Someone else takes the last units.
    let left = crate::app::catalog::available_stock(&store, &product_id).await?;
    inventory
        .reserve_stock(&stock_item, "order-elsewhere", left)
        .await?;
    let guest = text(Browser::new(&router).get(&product).await).await?;
    assert!(guest.contains("Rupture"), "{guest}");
    assert!(
        guest.contains("pour être alerté du retour en stock"),
        "{guest}"
    );

    let page = text(browser.get(&product).await).await?;
    assert!(
        page.contains("alerter du retour en stock</button>"),
        "{page}"
    );
    let asked = browser.post(&format!("{product}/alert"), "").await;
    assert_eq!(location(&asked), product);
    // Asking twice is harmless.
    let twice = browser.post(&format!("{product}/alert"), "").await;
    assert_eq!(twice.status(), StatusCode::SEE_OTHER);
    db::run_subscriptions_once(&store).await?;
    let page = text(browser.get(&product).await).await?;
    assert!(page.contains("Alerte enregistrée"), "{page}");
    let waiting = text(browser.get("/account/alerts").await).await?;
    assert!(
        waiting.contains("En attente du retour en stock"),
        "{waiting}"
    );

    // Changed their mind, then asked again: cancelling is not final.
    let cancelled = browser
        .post(&format!("/account/alerts/{product_id}/cancel"), "")
        .await;
    assert_eq!(location(&cancelled), "/account/alerts");
    db::run_subscriptions_once(&store).await?;
    let none = text(browser.get("/account/alerts").await).await?;
    assert!(none.contains("Aucune alerte."), "{none}");
    let page = text(browser.get(&product).await).await?;
    assert!(
        page.contains("alerter du retour en stock</button>"),
        "{page}"
    );
    browser.post(&format!("{product}/alert"), "").await;
    db::run_subscriptions_once(&store).await?;
    let waiting = text(browser.get("/account/alerts").await).await?;
    assert!(waiting.contains("Supprimer l"), "{waiting}");

    // A delivery arrives: the alert fires and the shopper's list says so.
    inventory.receive_stock(&stock_item, 3).await?;
    db::run_subscriptions_once(&store).await?;
    let back = text(browser.get("/account/alerts").await).await?;
    assert!(back.contains("De nouveau disponible"), "{back}");
    let page = text(browser.get(&product).await).await?;
    assert!(page.contains("En stock"), "{page}");
    assert!(!page.contains("Alerte enregistrée"), "{page}");
    Ok(())
}

#[tokio::test]
async fn a_shopper_pays_at_the_shops_payment_provider() -> anyhow::Result<()> {
    let provider = timada_payment::FakeProvider::card_only();
    let (router, store, product_id) = shop_with(std::sync::Arc::new(provider.clone())).await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;

    // Only what the provider takes is offered — and accepted.
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("value=\"card\""), "{checkout}");
    assert!(!checkout.contains("value=\"installments\""), "{checkout}");
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();
    let form = |mode: &str| {
        format!("delivery_address_id={address_id}&delivery_method=colissimo&payment_mode={mode}")
    };
    let refused = browser.post("/checkout", &form("installments")).await;
    assert_eq!(refused.status(), StatusCode::OK);
    let refused = text(refused).await?;
    assert!(
        refused.contains("n&#x27;est pas proposé") || refused.contains("n'est pas proposé"),
        "{refused}"
    );

    let placed = browser.post("/checkout", &form("card")).await;
    let pay = location(&placed);
    assert!(pay.starts_with("/checkout/pay/"), "{pay}");
    let order_id = pay.rsplit('/').next().unwrap_or_default().to_owned();
    let confirmation = format!("/checkout/confirmation/{order_id}");

    // Stock first, then the payment is requested: the page waits for it.
    let waiting = text(browser.get(&pay).await).await?;
    assert!(waiting.contains("http-equiv=\"refresh\""), "{waiting}");
    db::run_subscriptions_once(&store).await?;
    let page = text(browser.get(&pay).await).await?;
    assert!(page.contains("Payer ma commande"), "{page}");
    assert!(page.contains("125,85 €"), "119,95 + 5,90: {page}");
    assert!(!page.contains("http-equiv=\"refresh\""), "{page}");
    let payment_id = timada_payment::payment_id(&order_id);
    let session = timada_payment::FakeProvider::session_of(&payment_id);
    assert!(page.contains(&session), "{page}");
    // Coming back reopens the same session; nothing is confirmed unpaid.
    assert!(text(browser.get(&pay).await).await?.contains(&session));
    let early = browser.get(&confirmation).await;
    assert_eq!(location(&early), pay);

    // The provider reports the payment: the order is paid and confirmed.
    let paid = timada_payment::ProviderEvent::Paid {
        payment_id: payment_id.clone(),
        reference: "pi_demo".into(),
        amount: timada_core::Money::eur(12_585),
    };
    let applied =
        timada_payment::apply_provider_event(&store.executor, &store.db, &provider, paid).await?;
    assert_eq!(applied, timada_payment::Applied::Done);
    db::run_subscriptions_once(&store).await?;
    let back = browser.get(&pay).await;
    assert_eq!(location(&back), confirmation);
    let done = text(browser.get(&confirmation).await).await?;
    assert!(done.contains("votre commande est confirmée"), "{done}");

    // Someone else's order is nobody's business.
    let mut stranger = Browser::new(&router);
    stranger
        .post(
            "/register",
            &REGISTER.replace("ada%40example.com", "eve%40example.com"),
        )
        .await;
    assert_eq!(stranger.get(&pay).await.status(), StatusCode::NOT_FOUND);

    // A cancelled order's money goes back through the provider.
    timada_order::Command(&store.executor)
        .cancel_order(&order_id, "customer changed mind")
        .await?;
    db::run_subscriptions_once(&store).await?;
    let sent_back = provider.refunds();
    assert_eq!(sent_back.len(), 1, "{sent_back:?}");
    assert_eq!(sent_back[0].psp_reference, "pi_demo");
    assert_eq!(sent_back[0].amount, timada_core::Money::eur(12_585));
    Ok(())
}

/// Checks out one unit with the seeded shopper; the path of the payment step.
async fn place_card_order(
    browser: &mut Browser<'_>,
    store: &Store,
    product_id: &str,
) -> anyhow::Result<String> {
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    let checkout = text(browser.get("/checkout").await).await?;
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();
    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    db::run_subscriptions_once(store).await?;
    Ok(location(&placed))
}

#[tokio::test]
async fn the_card_form_is_embedded_under_a_policy_naming_the_provider() -> anyhow::Result<()> {
    let provider = timada_payment::FakeProvider::embedded();
    let (router, store, product_id) = shop_with(std::sync::Arc::new(provider)).await?;
    let mut browser = Browser::new(&router);
    let pay = place_card_order(&mut browser, &store, &product_id).await?;
    let order_id = pay.rsplit('/').next().unwrap_or_default().to_owned();
    let session = timada_payment::FakeProvider::session_of(&timada_payment::payment_id(&order_id));

    let response = browser.get(&pay).await;
    let policy = response
        .headers()
        .get("content-security-policy")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    assert!(
        policy.contains("script-src 'self' https://js.stripe.com;"),
        "{policy}"
    );
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    assert!(!policy.contains("unsafe-eval"), "{policy}");
    let page = text(response).await?;
    assert!(
        page.contains(&format!("data-client-secret=\"{session}_secret\"")),
        "{page}"
    );
    assert!(page.contains("data-publishable-key=\"pk_fake\""), "{page}");
    assert!(
        page.contains(&format!("data-return-url=\"http://127.0.0.1:3000{pay}\"")),
        "{page}"
    );
    assert!(page.contains("Payer 125,85 €"), "{page}");
    assert!(page.contains("src=\"https://js.stripe.com/v3/\""), "{page}");
    assert!(page.contains("src=\"/checkout/pay.js\""), "{page}");
    // Nothing inline to allow: the page's own script is a file of the shop.
    assert!(!page.contains("<script>"), "{page}");
    // The form does its own waiting; a reload would wipe a card being typed.
    assert!(!page.contains("http-equiv=\"refresh\""), "{page}");

    let script = browser.get("/checkout/pay.js").await;
    assert_eq!(
        script
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/javascript; charset=utf-8")
    );
    assert!(text(script).await?.contains("confirmPayment"));

    // The other pages carry no such policy (and load nothing from outside).
    let home = browser.get("/").await;
    assert!(home.headers().get("content-security-policy").is_none());
    Ok(())
}

#[cfg(feature = "stripe")]
#[tokio::test]
async fn stripe_webhooks_are_verified_then_capture_the_payment() -> anyhow::Result<()> {
    use http::Request;
    use topcoat::router::Body;

    let (_, mut store, product_id) =
        shop_with(std::sync::Arc::new(timada_payment::FakeProvider::embedded())).await?;
    // Only the webhook's signature is Stripe's here: nothing calls its API.
    let mut config = timada_payment::StripeConfig::new("sk_test_x", "pk_test_x", "whsec_demo");
    config.api_base = "http://127.0.0.1:9".into();
    store.stripe = Some(std::sync::Arc::new(timada_payment::StripeProvider::new(
        config,
    )?));
    let router = crate::router(
        store.clone(),
        AssetConfig::hosted_at("/assets", AssetCatalog::default()),
        Stylesheet::Url("/dev.css".into()),
    );
    let mut browser = Browser::new(&router);
    let pay = place_card_order(&mut browser, &store, &product_id).await?;
    let order_id = pay.rsplit('/').next().unwrap_or_default().to_owned();
    let payment_id = timada_payment::payment_id(&order_id);

    let payload = format!(
        r#"{{"type":"payment_intent.succeeded","data":{{"object":{{"id":"pi_demo","amount_received":12585,"currency":"eur","metadata":{{"payment_id":"{payment_id}"}}}}}}}}"#
    );
    let now = timada_core::time::now_unix_secs()?;
    let deliver = |signature: String, body: String| {
        let request = Request::builder()
            .method("POST")
            .uri("/webhooks/stripe")
            .header("content-type", "application/json")
            .header("stripe-signature", signature)
            .body(Body::from(body.into_bytes()));
        let router = &router;
        async move { Ok::<_, anyhow::Error>(router.handle(request?).await.status()) }
    };

    // Anyone can post to a webhook: only Stripe's signature is believed.
    let forged = timada_payment::sign_webhook("whsec_guess", now, payload.as_bytes());
    assert_eq!(
        deliver(forged, payload.clone()).await?,
        StatusCode::BAD_REQUEST
    );
    let replayed = timada_payment::sign_webhook("whsec_demo", now - 3_600, payload.as_bytes());
    assert_eq!(
        deliver(replayed, payload.clone()).await?,
        StatusCode::BAD_REQUEST
    );
    let signed = timada_payment::sign_webhook("whsec_demo", now, payload.as_bytes());
    let tampered = payload.replace("12585", "1");
    assert_eq!(
        deliver(signed.clone(), tampered).await?,
        StatusCode::BAD_REQUEST
    );
    let untouched = timada_payment::load_payment(&store.executor, &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(untouched.status, timada_payment::PaymentStatus::Requested);

    // Stripe's own word captures — once, however often it is delivered.
    for _ in 0..2 {
        assert_eq!(
            deliver(signed.clone(), payload.clone()).await?,
            StatusCode::OK
        );
    }
    let payment = timada_payment::load_payment(&store.executor, &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.status, timada_payment::PaymentStatus::Captured);
    assert_eq!(payment.psp_reference.as_deref(), Some("pi_demo"));
    // Events the shop has no use for are acknowledged, or Stripe insists.
    let other = r#"{"type":"customer.created","data":{"object":{}}}"#.to_owned();
    let signature = timada_payment::sign_webhook("whsec_demo", now, other.as_bytes());
    assert_eq!(deliver(signature, other).await?, StatusCode::OK);

    db::run_subscriptions_once(&store).await?;
    let back = browser.get(&pay).await;
    assert_eq!(
        location(&back),
        format!("/checkout/confirmation/{order_id}")
    );

    // Weeks later the cardholder contests the charge, and the bank sides
    // with the shop: Stripe says
    // so against its own reference, which the shop learnt at the capture.
    let report = |status: &str| {
        format!(
            r#"{{"type":"charge.dispute.closed","data":{{"object":{{"id":"dp_demo","amount":12585,"currency":"eur","status":"{status}","reason":"fraudulent","payment_intent":"pi_demo","evidence_details":{{"due_by":1800000000}}}}}}}}"#
        )
    };
    let opened = report("needs_response");
    let signature = timada_payment::sign_webhook("whsec_demo", now, opened.as_bytes());
    assert_eq!(deliver(signature, opened).await?, StatusCode::OK);
    let payment = timada_payment::load_payment(&store.executor, &payment_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(
        payment.open_dispute().map(|d| d.dispute_id.as_str()),
        Some("dp_demo")
    );
    let won = report("won");
    let signature = timada_payment::sign_webhook("whsec_demo", now, won.as_bytes());
    assert_eq!(deliver(signature, won).await?, StatusCode::OK);
    db::run_subscriptions_once(&store).await?;
    let disputes = timada_payment::disputes_of_order(&store.db, &order_id).await?;
    assert_eq!(disputes.len(), 1);
    assert_eq!(disputes[0].status, "won");
    Ok(())
}

/// The order of the product links of a listing page.
fn listed(page: &str) -> Vec<String> {
    page.split("<h2><a href=\"/p/")
        .skip(1)
        .filter_map(|rest| rest.split_once("\">"))
        .filter_map(|(_, rest)| rest.split_once("</a>"))
        .map(|(name, _)| name.replace("&quot;", "\""))
        .collect()
}

#[tokio::test]
async fn a_full_catalogue_is_listed_filtered_searched_and_mapped() -> anyhow::Result<()> {
    let (router, store, _) = shop().await?;
    crate::seed_catalogue::run(&store).await?;
    // Seeding twice adds nothing.
    crate::seed_catalogue::run(&store).await?;
    db::run_subscriptions_once(&store).await?;
    let mut browser = Browser::new(&router);

    // The plain listing: paged, each page with an address of its own to index.
    let home = text(browser.get("/").await).await?;
    assert!(home.contains(">25 produits<"), "{home}");
    assert_eq!(listed(&home).len(), 24);
    assert!(
        home.contains("<link rel=\"canonical\" href=\"http://127.0.0.1:3000/\">"),
        "{home}"
    );
    assert!(
        home.contains("<link rel=\"next\" href=\"http://127.0.0.1:3000/?page=2\">"),
        "{home}"
    );
    assert!(!home.contains("name=\"robots\""), "{home}");
    assert!(home.contains("<img src=\"/media/demo/jbl-flip6.svg\" alt=\"JBL Flip 6\" width=\"240\" height=\"240\" loading=\"lazy\""), "{home}");
    let last = text(browser.get("/?page=7").await).await?;
    assert_eq!(listed(&last).len(), 1, "a page past the end is the last");
    assert!(
        last.contains("href=\"http://127.0.0.1:3000/?page=2\""),
        "{last}"
    );
    assert!(last.contains("rel=\"prev\""), "{last}");

    // Filters and sort come from the query string; such a page is a
    // variation, not something to index.
    let logitech = text(
        browser
            .get("/?marque=logitech&stock=1&tri=prix-croissant")
            .await,
    )
    .await?;
    assert_eq!(
        listed(&logitech),
        [
            "Logitech G502 X",
            "Logitech MX Master 3S",
            "Logitech MX Keys S",
            "Logitech G915 TKL"
        ]
    );
    assert!(
        logitech.contains("content=\"noindex,follow\""),
        "{logitech}"
    );
    assert!(!logitech.contains("rel=\"canonical\""), "{logitech}");
    assert!(
        logitech.contains("value=\"logitech\" checked"),
        "{logitech}"
    );
    // The other brands still say what they would add.
    assert!(logitech.contains("Corsair (3)"), "{logitech}");
    assert!(logitech.contains("Tout afficher"), "{logitech}");
    let two_brands = text(browser.get("/?marque=jbl&marque=sony&note=4").await).await?;
    assert_eq!(listed(&two_brands).len(), 3, "{two_brands}");
    let cheap = text(
        browser
            .get("/?prix_max=80&tri=prix-decroissant&page=zzz")
            .await,
    )
    .await?;
    assert_eq!(
        listed(&cheap),
        [
            "Logitech G502 X",
            "Philips Hue Go",
            "Crucial P3 Plus 1 To",
            "Corsair M65 RGB Ultra",
            "Corsair K55 Core"
        ]
    );

    // Search: accents folded, unfinished words, the category's name counts.
    let search = text(browser.get("/recherche?q=ecran+incurve").await).await?;
    let mut found = listed(&search);
    found.sort();
    assert_eq!(
        found,
        [
            "LG 34\" UltraWide incurvé 34WP65C",
            "Samsung 32\" Odyssey G5"
        ]
    );
    assert!(search.contains("Recherche : ecran incurve"), "{search}");
    assert!(search.contains("content=\"noindex,follow\""), "{search}");
    let nothing = text(browser.get("/recherche?q=trottinette").await).await?;
    assert!(
        nothing.contains("Aucun produit ne correspond."),
        "{nothing}"
    );
    // Nothing a shopper types is markup or a query operator.
    let odd = browser.get("/recherche?q=%22%3Cscript%3E+OR+*").await;
    assert_eq!(odd.status(), StatusCode::OK);
    let odd = text(odd).await?;
    // As text it is escaped; inside the quoted attribute only the quote
    // could do harm, and it is escaped too.
    assert!(
        odd.contains("<h1>Recherche : \"&lt;script&gt; OR *</h1>"),
        "{odd}"
    );
    assert!(odd.contains("value=\"&quot;<script> OR *\""), "{odd}");
    assert!(odd.contains("Aucun produit ne correspond."), "{odd}");

    // A branch of the tree, a brand.
    let components = text(browser.get("/c/composants").await).await?;
    assert!(components.contains(">6 produits<"), "{components}");
    assert!(components.contains("href=\"/c/ssd\""), "{components}");
    assert!(components.contains("placeholder=\"74\""), "{components}");
    assert!(components.contains("placeholder=\"660\""), "{components}");
    let jbl = text(browser.get("/marque/jbl").await).await?;
    assert_eq!(listed(&jbl), ["JBL Charge 5", "JBL Flip 6"]);
    assert!(!jbl.contains("<legend>Marque</legend>"), "{jbl}");
    assert!(jbl.contains("Rupture"), "{jbl}");
    assert!(jbl.contains("5,0 / 5 (1 avis)"), "{jbl}");
    assert!(jbl.contains("\"@type\":\"BreadcrumbList\""), "{jbl}");
    assert_eq!(
        browser.get("/marque/nokia").await.status(),
        StatusCode::NOT_FOUND
    );

    // A category is filtered by the lines of the technical sheet its operator
    // picked, each value counted within the other picks.
    let screens = text(browser.get("/c/ecran-pc").await).await?;
    assert!(screens.contains(">7 produits<"), "{screens}");
    for offered in [
        "<legend>Taille</legend>",
        "24 pouces (2)",
        "IPS (4)",
        "VA (2)",
        "60 Hz",
    ] {
        assert_eq!(
            screens.contains(offered),
            offered != "<span>60 Hz",
            "{offered}: {screens}"
        );
    }
    // By their number: 75 Hz before 144 Hz before 165 Hz.
    let hz = |value: &str| screens.find(value).unwrap_or(usize::MAX);
    assert!(hz("75 Hz (1)") < hz("144 Hz (2)") && hz("144 Hz (2)") < hz("165 Hz (2)"));
    let fast_va = text(
        browser
            .get("/c/ecran-pc?f_dalle-type=VA&f_dalle-frequence=144+Hz")
            .await,
    )
    .await?;
    assert_eq!(listed(&fast_va), ["Samsung 32\" Odyssey G5"]);
    assert!(
        fast_va.contains("name=\"f_dalle-type\" value=\"VA\" checked"),
        "{fast_va}"
    );
    assert!(
        fast_va.contains("IPS (1)"),
        "the other panel among the 144 Hz ones: {fast_va}"
    );
    assert!(fast_va.contains("content=\"noindex,follow\""), "{fast_va}");
    // Above, nobody picked filters: the same parameter means nothing there.
    let above = text(browser.get("/c/ecran-ordinateur?f_dalle-type=VA").await).await?;
    assert!(above.contains(">7 produits<"), "{above}");
    assert!(!above.contains("<legend>Taille</legend>"), "{above}");
    assert!(above.contains("rel=\"canonical\""), "{above}");
    // The sheet itself is on the product's page.
    let ssd = timada_catalog::product_id("CRU-P3-1T");
    let sheet = text(browser.get(&format!("/p/{ssd}")).await).await?;
    assert!(
        sheet.contains("<h2 id=\"fiche-technique\">Fiche technique</h2>"),
        "{sheet}"
    );
    assert!(
        sheet.contains("<th scope=\"row\">Interface</th><td>NVMe PCIe 4.0</td>"),
        "{sheet}"
    );

    // What search engines are handed.
    let sitemap = browser.get("/sitemap.xml").await;
    assert_eq!(sitemap.status(), StatusCode::OK);
    let sitemap = text(sitemap).await?;
    assert!(
        sitemap.contains("<loc>http://127.0.0.1:3000/c/carte-graphique</loc>"),
        "{sitemap}"
    );
    assert_eq!(sitemap.matches("/p/").count(), 25, "{sitemap}");
    assert!(sitemap.contains("<lastmod>"), "{sitemap}");
    let robots = text(browser.get("/robots.txt").await).await?;
    assert!(robots.contains("Disallow: /checkout"), "{robots}");
    assert!(
        robots.contains("Sitemap: http://127.0.0.1:3000/sitemap.xml"),
        "{robots}"
    );

    // The demo's drawn product pictures.
    let picture = browser.get("/media/demo/jbl-flip6.svg").await;
    assert_eq!(
        picture
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/svg+xml; charset=utf-8")
    );
    assert!(text(picture).await?.contains(">JBL-FLIP6</text>"));
    for bad in [
        "/media/demo/JBL.svg",
        "/media/demo/x%3Cy.svg",
        "/media/demo/jbl-flip6.png",
    ] {
        assert_eq!(
            browser.get(bad).await.status(),
            StatusCode::NOT_FOUND,
            "{bad}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_business_gives_its_vat_number_and_sees_what_the_registry_said() -> anyhow::Result<()> {
    let registry = timada_tax::FakeValidator::default();
    let (router, store, _) = shop_checking(
        std::sync::Arc::new(timada_payment::ManualProvider),
        registry.clone(),
    )
    .await?;
    let mut browser = Browser::new(&router);
    // Signed-in shoppers only.
    assert_eq!(
        browser.get("/account/company").await.status(),
        StatusCode::SEE_OTHER
    );
    browser.post("/register", REGISTER).await;
    let account = text(browser.get("/account").await).await?;
    assert!(account.contains("href=\"/account/company\""), "{account}");

    // A number that does not read like one is told why; nothing is kept.
    let typo = browser
        .post(
            "/account/company",
            "company_name=Analytical+Engines+GmbH&vat_number=DE12345",
        )
        .await;
    assert_eq!(typo.status(), StatusCode::OK);
    let typo = text(typo).await?;
    assert!(
        typo.contains("n&#x27;a pas la forme") || typo.contains("n'a pas la forme"),
        "{typo}"
    );
    assert!(
        typo.contains("value=\"DE12345\""),
        "what was typed stays: {typo}"
    );
    let british = text(
        browser
            .post(
                "/account/company",
                "company_name=Engines+Ltd&vat_number=GB123456789",
            )
            .await,
    )
    .await?;
    assert!(british.contains("« GB »"), "{british}");

    // Read as typed, checked at once, the proof shown.
    let saved = browser
        .post(
            "/account/company",
            "company_name=Analytical+Engines+GmbH&vat_number=de+123+456+789",
        )
        .await;
    assert_eq!(location(&saved), "/account/company");
    let page = text(browser.get("/account/company").await).await?;
    assert!(page.contains("value=\"DE123456789\""), "{page}");
    assert!(page.contains("Numéro valide"), "{page}");
    assert!(
        page.contains("consultation n° FAKE-DE123456789-1"),
        "{page}"
    );
    assert_eq!(registry.checks(), 1);

    // The registry is down: the last answer stands.
    registry.set_down(true);
    browser.post("/account/company/check", "").await;
    let page = text(browser.get("/account/company").await).await?;
    assert!(page.contains("FAKE-DE123456789-1"), "{page}");
    registry.set_down(false);

    // Struck off: said plainly.
    let number = timada_tax::VatNumber::parse("DE123456789")?;
    registry.reject(&number);
    browser.post("/account/company/check", "").await;
    let page = text(browser.get("/account/company").await).await?;
    assert!(page.contains("ne connaît pas ce numéro"), "{page}");

    // The operator sees the same.
    db::run_subscriptions_once(&store).await?;
    let ada =
        timada_customer::list_customers(&store.db, &timada_customer::ListCustomers::default())
            .await?
            .into_iter()
            .find(|customer| customer.email == "ada@example.com")
            .ok_or_else(|| anyhow::anyhow!("customer not listed"))?;
    let company = timada_customer::load_company_identity(&store.executor, &ada.customer_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("customer missing"))?;
    assert_eq!(company.company_name, "Analytical Engines GmbH");
    assert!(company.last_check.is_some_and(|check| !check.valid));

    // A consumer again.
    browser.post("/account/company/remove", "").await;
    let page = text(browser.get("/account/company").await).await?;
    assert!(!page.contains("DE123456789"), "{page}");
    assert!(!page.contains("Vérifier à nouveau"), "{page}");
    Ok(())
}

#[tokio::test]
async fn a_business_of_another_member_state_checks_out_without_vat() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    browser.post("/register", REGISTER).await;
    browser
        .post(
            "/account/addresses/new",
            &ADDRESS.replace("country_code=fr", "country_code=de"),
        )
        .await;
    let company = "company_name=Analytical+Engines+GmbH&vat_number=DE123456789";

    // A consumer delivered in Germany pays German VAT: 99,96 + 19 %.
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("118,95 €"), "{checkout}");
    assert!(
        checkout.contains("name=\"regime\" value=\"\""),
        "{checkout}"
    );

    // As a business with a valid number: the pre-tax price, and why.
    browser.post("/account/company", company).await;
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("99,96 €"), "{checkout}");
    assert!(
        checkout.contains("Analytical Engines GmbH (DE123456789)"),
        "{checkout}"
    );
    assert!(checkout.contains("autoliquidée"), "{checkout}");
    assert!(
        checkout.contains("name=\"regime\" value=\"autoliquidation\""),
        "{checkout}"
    );
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address"))?
        .to_owned();
    let form = format!(
        "delivery_address_id={address_id}&delivery_method=colissimo-europe&payment_mode=card&regime=autoliquidation"
    );

    // The standing changes between the page and the click: never an order at
    // a total nobody saw.
    browser.post("/account/company/remove", "").await;
    let refused = browser.post("/checkout", &form).await;
    assert_eq!(refused.status(), StatusCode::OK);
    let refused = text(refused).await?;
    assert!(refused.contains("Vérifiez le nouveau total"), "{refused}");
    assert!(refused.contains("118,95 €"), "{refused}");

    browser.post("/account/company", company).await;
    let placed = browser.post("/checkout", &form).await;
    assert_eq!(placed.status(), StatusCode::SEE_OTHER);
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;

    // 99,96 + 10,75 of delivery, no VAT — and the order says who owes it.
    let order = timada_order::load_order_details(&store.executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.total, timada_core::Money::eur(11_071));
    assert!(order.reverse_charge.is_some());
    let detail = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(
        detail.contains("Autoliquidation de la TVA par le preneur"),
        "{detail}"
    );
    assert!(detail.contains("110,71 €"), "{detail}");
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    let confirmation = outbox
        .iter()
        .find(|m| m.kind == "order-confirmation" && m.recipient == "ada@example.com")
        .ok_or_else(|| anyhow::anyhow!("no confirmation: {outbox:?}"))?;
    assert!(
        confirmation.body.contains("TVA autoliquidée"),
        "{}",
        confirmation.body
    );

    // Paid: the invoice names the business, and the quarter's VAT report
    // lists the sale apart from exports and from the one-stop shop.
    timada_payment::Command(&store.executor)
        .capture_payment(timada_payment::payment_id(&order_id), "psp-b2b".into())
        .await?;
    db::run_subscriptions_once(&store).await?;
    let invoice = text(
        browser
            .get(&format!("/account/orders/{order_id}/invoice"))
            .await,
    )
    .await?;
    assert!(invoice.contains("Analytical Engines GmbH"), "{invoice}");
    assert!(invoice.contains("N° TVA : DE123456789"), "{invoice}");
    assert!(invoice.contains("article 262 ter I du CGI"), "{invoice}");
    let now = timada_invoice::VatPeriod::of(timada_core::time::now_unix_secs()?);
    let report = timada_invoice::vat_report(&store.db, now).await?;
    assert_eq!(report.intra_community.len(), 1);
    assert_eq!(report.intra_community[0].buyer_vat_number, "DE123456789");
    assert_eq!(report.intra_community[0].base_minor, 11_071);
    assert!(report.oss.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_shop_is_browsed_by_category() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);

    // The top of the tree is the way in.
    let home = text(browser.get("/").await).await?;
    assert!(home.contains("href=\"/c/informatique\""), "{home}");
    assert!(!home.contains("href=\"/c/ecran-pc\""), "{home}");

    // A category shows what is under it, products of its subcategories included.
    let department = text(browser.get("/c/informatique").await).await?;
    assert!(
        department.contains("href=\"/c/peripheriques\""),
        "{department}"
    );
    assert!(
        department.contains(&format!("/p/{product_id}")),
        "{department}"
    );
    assert!(department.contains(">1 produit<"), "{department}");
    // Each way further down says how much it holds.
    assert!(
        department.contains(">Périphériques (1)</a>"),
        "{department}"
    );

    // The trail links every step but the page itself.
    let leaf = text(browser.get("/c/ecran-pc?page=9").await).await?;
    assert!(
        leaf.contains("aria-label=\"Fil d&#x27;Ariane\"")
            || leaf.contains("aria-label=\"Fil d'Ariane\""),
        "{leaf}"
    );
    assert!(leaf.contains("<a href=\"/c/ecran-ordinateur\">"), "{leaf}");
    assert!(
        leaf.contains("<li aria-current=\"page\">Écran PC</li>"),
        "{leaf}"
    );
    assert!(leaf.contains(&format!("/p/{product_id}")), "{leaf}");

    // From a product, the way back up — its own category linked too — and
    // one address for the product, whatever page of its reviews is shown.
    let product = text(browser.get(&format!("/p/{product_id}?avis=2")).await).await?;
    assert!(
        product.contains(&format!(
            "<link rel=\"canonical\" href=\"http://127.0.0.1:3000/p/{product_id}\">"
        )),
        "{product}"
    );
    assert!(product.contains("\"position\":6"), "{product}");
    // …and what is on offer: the product, its price, that it can be had.
    assert!(product.contains("\"@type\":\"Product\""), "{product}");
    assert!(
        product.contains("\"price\":\"119.95\",\"priceCurrency\":\"EUR\",\"availability\":\"https://schema.org/InStock\""),
        "{product}"
    );
    assert!(
        product.contains("<a href=\"/c/ecran-pc\">Écran PC</a>"),
        "{product}"
    );

    // An archived branch leaves the shop with what is under it; the product
    // stays on sale, under the label it was created with.
    timada_catalog::Command(&store.executor)
        .archive_category(timada_catalog::category_id("peripheriques"))
        .await?;
    db::run_subscriptions_once(&store).await?;
    for gone in ["/c/peripheriques", "/c/ecran-pc", "/c/nowhere"] {
        assert_eq!(
            browser.get(gone).await.status(),
            StatusCode::NOT_FOUND,
            "{gone}"
        );
    }
    let department = text(browser.get("/c/informatique").await).await?;
    assert!(!department.contains("/c/peripheriques"), "{department}");
    // Still on sale: listed under what is left of its branch.
    assert!(
        department.contains(&format!("/p/{product_id}")),
        "{department}"
    );
    let product = browser.get(&format!("/p/{product_id}")).await;
    assert_eq!(product.status(), StatusCode::OK);
    let product = text(product).await?;
    assert!(!product.contains("/c/ecran-pc"), "{product}");
    assert!(
        product.contains("Informatique &gt; Périphériques")
            || product.contains("Informatique > Périphériques"),
        "{product}"
    );
    Ok(())
}

#[tokio::test]
async fn an_order_left_unpaid_is_cancelled_and_the_shopper_told() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=2"))
        .await;
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    let checkout = text(browser.get("/checkout").await).await?;
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();
    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;
    let before = crate::app::catalog::available_stock(&store, &product_id).await?;

    // Nobody completes the payment: the sweep expires it.
    let now = timada_core::time::now_unix_secs()?;
    // Two orders expire: this one and the seeded order, which nobody paid either.
    let expired = timada_order::expire_unpaid_orders(
        &store.executor,
        &store.db,
        store.provider.as_ref(),
        now + 1,
    )
    .await?;
    assert_eq!(expired, 2);
    db::run_subscriptions_once(&store).await?;

    let after = crate::app::catalog::available_stock(&store, &product_id).await?;
    assert!(after >= before + 2, "{before} → {after}");
    // The payment step, left open in a tab, says so too.
    let pay = text(browser.get(&format!("/checkout/pay/{order_id}")).await).await?;
    assert!(pay.contains("Votre commande a été annulée"), "{pay}");
    assert!(
        pay.contains("paiement non finalisé dans les délais"),
        "{pay}"
    );
    let detail = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(detail.contains("Annulée"), "{detail}");
    assert!(
        detail.contains("paiement non finalisé dans les délais"),
        "{detail}"
    );
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    let cancelled = outbox
        .iter()
        .find(|m| m.kind == "order-cancelled" && m.recipient == "ada@example.com")
        .ok_or_else(|| anyhow::anyhow!("no cancellation e-mail: {outbox:?}"))?;
    assert!(
        cancelled
            .body
            .contains("paiement non finalisé dans les délais"),
        "{}",
        cancelled.body
    );
    Ok(())
}

#[tokio::test]
async fn a_shipped_order_is_returned_from_the_account() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=2"))
        .await;
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    let checkout = text(browser.get("/checkout").await).await?;
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address radio"))?
        .to_owned();
    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    let order_page = format!("/account/orders/{order_id}");
    db::run_subscriptions_once(&store).await?;

    // Not shipped yet: nothing to return.
    let page = text(browser.get(&order_page).await).await?;
    assert!(!page.contains("Retourner des articles"), "{page}");
    // Not paid yet: the invoice is a draft, there is nothing to download.
    assert!(!page.contains("Télécharger la facture"), "{page}");
    for uri in [
        format!("{order_page}/invoice"),
        format!("{order_page}/invoice.pdf"),
    ] {
        assert_eq!(browser.get(&uri).await.status(), StatusCode::NOT_FOUND);
    }
    timada_payment::Command(&store.executor)
        .capture_payment(timada_payment::payment_id(&order_id), "psp-1".into())
        .await?;
    db::run_subscriptions_once(&store).await?;
    timada_shipping::Command(&store.executor)
        .dispatch_shipment(
            timada_shipping::shipment_id(&order_id),
            "Colissimo".into(),
            "XY123".into(),
        )
        .await?;
    db::run_subscriptions_once(&store).await?;

    let page = text(browser.get(&order_page).await).await?;
    assert!(page.contains("Retourner des articles"), "{page}");
    let form_uri = format!("{order_page}/return");
    let form = text(browser.get(&form_uri).await).await?;
    assert!(form.contains("Quantité à retourner"), "{form}");

    // Nothing ticked, then more than was bought: both come back with a message.
    let nothing = browser
        .post(
            &form_uri,
            &format!("product_0={product_id}&quantity_0=0&ground=other"),
        )
        .await;
    assert!(text(nothing).await?.contains("au moins un article"));
    let greedy = browser
        .post(
            &form_uri,
            &format!("product_0={product_id}&quantity_0=3&ground=other"),
        )
        .await;
    assert!(text(greedy).await?.contains("plus retourner que 2"));

    let asked = browser
        .post(
            &form_uri,
            &format!("product_0={product_id}&quantity_0=1&ground=changed-mind&details=Trop+grand"),
        )
        .await;
    let slip_uri = location(&asked);
    assert!(slip_uri.starts_with("/account/returns/"), "{slip_uri}");
    let return_id = slip_uri.rsplit('/').next().unwrap_or_default().to_owned();
    db::run_subscriptions_once(&store).await?;
    let slip = text(browser.get(&slip_uri).await).await?;
    assert!(slip.contains("Demande en cours d"), "{slip}");
    assert!(
        slip.contains("Ne convient pas / changement d'avis — Trop grand"),
        "{slip}"
    );
    // Someone else cannot read it.
    let mut other = Browser::new(&router);
    other
        .post(
            "/login",
            &format!(
                "email={}&password={}",
                seed::SHOPPER_EMAIL.replace('@', "%40"),
                seed::SHOPPER_PASSWORD
            ),
        )
        .await;
    assert_eq!(other.get(&slip_uri).await.status(), StatusCode::NOT_FOUND);

    // Approved: the slip gives the address; received: refunded and restocked.
    let returns = timada_returns::Command {
        executor: &store.executor,
        db: store.db.clone(),
        policy: db::return_policy(),
    };
    // The form said what the way back costs before the request was made.
    assert!(form.contains("6,90 € sont déduits"), "{form}");
    returns
        .approve_return_with_label(
            &return_id,
            timada_returns::IssueLabel {
                carrier: "Colissimo".into(),
                tracking_number: "8R0001".into(),
                url: None,
                file: Some(timada_returns::LabelFile {
                    file_name: "etiquette.pdf".into(),
                    content_type: "application/pdf".into(),
                    bytes: b"%PDF-1.4 etiquette".to_vec(),
                }),
                waive_fee: false,
            },
        )
        .await?;
    db::run_subscriptions_once(&store).await?;
    let slip = text(browser.get(&slip_uri).await).await?;
    assert!(slip.contains("Envoyer votre colis"), "{slip}");
    assert!(slip.contains("Service retours"), "{slip}");
    // A change of mind: the prepaid label is the customer's to pay.
    assert!(slip.contains("Colissimo, suivi 8R0001"), "{slip}");
    assert!(slip.contains("6,90 €, est déduit"), "{slip}");
    let label_uri = format!("{slip_uri}/label");
    assert!(slip.contains(&label_uri), "{slip}");
    let label = browser.get(&label_uri).await;
    assert_eq!(label.status(), StatusCode::OK);
    assert_eq!(
        label
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/pdf")
    );
    let label = to_bytes(label.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    assert_eq!(label.as_ref(), b"%PDF-1.4 etiquette");
    assert_eq!(other.get(&label_uri).await.status(), StatusCode::NOT_FOUND);
    let stock_before = crate::app::catalog::available_stock(&store, &product_id).await?;
    returns
        .receive_return(
            &return_id,
            timada_returns::ReceiveReturn {
                lines: vec![timada_returns::ReceivedLine {
                    product_id: product_id.clone(),
                    accepted: 1,
                    restock: true,
                }],
                refund_method: timada_returns::RefundMethod::OriginalPayment,
                replace: false,
            },
        )
        .await?;
    db::run_subscriptions_once(&store).await?;

    let slip = text(browser.get(&slip_uri).await).await?;
    assert!(slip.contains("Traité"), "{slip}");
    // 119,95 € less the label.
    assert!(slip.contains("113,05 €"), "{slip}");
    assert!(slip.contains("6,90 € déduits du remboursement"), "{slip}");
    assert_eq!(
        crate::app::catalog::available_stock(&store, &product_id).await?,
        stock_before + 1
    );
    // The invoice, print-ready: seller, buyer, VAT, and the credit note of
    // the refund. Nobody else can open it.
    let invoice_uri = format!("{order_page}/invoice");
    let invoice = browser.get(&invoice_uri).await;
    assert_eq!(invoice.status(), StatusCode::OK);
    let invoice = text(invoice).await?;
    assert!(invoice.contains("Timada demo SAS"), "{invoice}");
    assert!(invoice.contains("SIRET"), "{invoice}");
    assert!(invoice.contains("12 rue des Machines"), "{invoice}");
    assert!(invoice.contains("TVA par taux"), "{invoice}");
    assert!(
        invoice.contains("Avoirs émis sur cette facture"),
        "{invoice}"
    );
    assert!(invoice.contains("@media print"), "{invoice}");
    assert!(invoice.contains("Télécharger le PDF"), "{invoice}");
    assert_eq!(
        other.get(&invoice_uri).await.status(),
        StatusCode::NOT_FOUND
    );
    // The same document as a file, for its owner only.
    let pdf_uri = format!("{order_page}/invoice.pdf");
    let pdf = browser.get(&pdf_uri).await;
    assert_eq!(pdf.status(), StatusCode::OK);
    let header = |name: &str| {
        pdf.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    assert_eq!(header("content-type"), "application/pdf");
    assert!(
        header("content-disposition").starts_with("attachment; filename=\"facture-F"),
        "{}",
        header("content-disposition")
    );
    assert_eq!(header("cache-control"), "private, no-store");
    let downloaded = to_bytes(pdf.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    assert!(downloaded.starts_with(b"%PDF-"));
    assert_eq!(other.get(&pdf_uri).await.status(), StatusCode::NOT_FOUND);
    // It is the archived file — the invoice as issued, filed once — and it
    // stays the same download after download, credit notes or not.
    let invoice_id = timada_invoice::invoice_id(order_page.rsplit('/').next().unwrap_or_default());
    let (entry, archived) =
        timada_invoice::read_archived(&store.db, store.archive.0.as_ref(), &invoice_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("invoice not archived"))?;
    assert_eq!(downloaded.as_ref(), archived.as_slice());
    assert_eq!(entry.sha256, timada_invoice::sha256_hex(&downloaded));
    assert!(!entry.reconstituted);
    let again = to_bytes(browser.get(&pdf_uri).await.into_body(), usize::MAX)
        .await
        .map_err(|e| anyhow::anyhow!("{e:#}"))?;
    assert_eq!(again, downloaded);

    // The order page lists the return, shows the refund and its credit note,
    // and still offers to return the unit that is left.
    let page = text(browser.get(&order_page).await).await?;
    assert!(page.contains("Retours de cette commande"), "{page}");
    assert!(page.contains("Télécharger la facture (PDF)"), "{page}");
    assert!(page.contains(&pdf_uri), "{page}");
    assert!(page.contains("Version imprimable"), "{page}");
    assert!(page.contains("Remboursé"), "{page}");
    assert!(page.contains("Avoirs émis"), "{page}");
    assert!(page.contains("Retourner des articles"), "{page}");

    // The shopper was written to at each step.
    let outbox = timada_mailer::list_outbox(&store.db, None, 100, 0).await?;
    let kinds: Vec<&str> = outbox
        .iter()
        .filter(|m| m.recipient == "ada@example.com")
        .map(|m| m.kind.as_str())
        .collect();
    for kind in [
        "return-approved",
        "return-completed",
        "refund",
        "invoice-issued",
    ] {
        assert!(kinds.contains(&kind), "{kind} missing from {kinds:?}");
    }
    // The invoice went out as a file, once the order was paid.
    let invoice_mail = outbox
        .iter()
        .find(|m| m.kind == "invoice-issued")
        .ok_or_else(|| anyhow::anyhow!("no invoice e-mail"))?;
    assert!(
        invoice_mail.subject.starts_with("Votre facture F"),
        "{}",
        invoice_mail.subject
    );
    let files = timada_mailer::outbox_attachments(&store.db, &invoice_mail.message_id).await?;
    assert_eq!(files.len(), 1, "{files:?}");
    assert!(files[0].file_name.starts_with("facture-F"), "{files:?}");
    assert!(files[0].size > 5_000, "{files:?}");
    // …the very file the account serves.
    assert_eq!(files[0].size, entry.size, "{files:?}");
    let approved = outbox
        .iter()
        .find(|m| m.kind == "return-approved")
        .ok_or_else(|| anyhow::anyhow!("no approval e-mail"))?;
    assert!(
        approved.body.contains("Service retours"),
        "{}",
        approved.body
    );

    // The other unit arrives broken: the shop sends the same product again
    // rather than refunding it.
    let asked = browser
        .post(
            &form_uri,
            &format!("product_0={product_id}&quantity_0=1&ground=defective"),
        )
        .await;
    let broken_uri = location(&asked);
    let broken_id = broken_uri.rsplit('/').next().unwrap_or_default().to_owned();
    returns.approve_return(&broken_id).await?;
    returns
        .receive_return(
            &broken_id,
            timada_returns::ReceiveReturn {
                lines: vec![timada_returns::ReceivedLine {
                    product_id: product_id.clone(),
                    accepted: 1,
                    restock: false,
                }],
                refund_method: timada_returns::RefundMethod::OriginalPayment,
                replace: true,
            },
        )
        .await?;
    let stock_before = crate::app::catalog::available_stock(&store, &product_id).await?;
    db::run_subscriptions_once(&store).await?;
    let slip = text(browser.get(&broken_uri).await).await?;
    assert!(slip.contains("Traité"), "{slip}");
    assert!(slip.contains("remplacement est en préparation"), "{slip}");
    assert!(!slip.contains("Remboursement sur votre moyen"), "{slip}");
    assert_eq!(
        crate::app::catalog::available_stock(&store, &product_id).await?,
        stock_before - 1
    );
    timada_shipping::Command(&store.executor)
        .dispatch_shipment(
            timada_shipping::replacement_shipment_id(&broken_id),
            "Colissimo".into(),
            "XY999".into(),
        )
        .await?;
    db::run_subscriptions_once(&store).await?;
    let slip = text(browser.get(&broken_uri).await).await?;
    assert!(slip.contains("remplacement est expédié"), "{slip}");
    assert!(slip.contains("suivi XY999"), "{slip}");
    // The order itself is not "shipped" a second time, and nothing more was
    // refunded.
    let payment =
        timada_payment::load_payment(&store.executor, timada_payment::payment_id(&order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.refunded, timada_core::Money::eur(11_305));
    let outbox = timada_mailer::list_outbox(&store.db, None, 100, 0).await?;
    let shipped: Vec<_> = outbox
        .iter()
        .filter(|m| m.kind == "replacement-shipped")
        .collect();
    assert_eq!(shipped.len(), 1, "{shipped:?}");
    assert!(shipped[0].body.contains("XY999"), "{}", shipped[0].body);
    let completed = outbox
        .iter()
        .filter(|m| m.kind == "return-completed")
        .find(|m| m.body.contains("même produit"))
        .ok_or_else(|| anyhow::anyhow!("no e-mail announcing the replacement"))?;
    assert!(
        !completed.body.contains("remboursement"),
        "{}",
        completed.body
    );
    Ok(())
}

#[tokio::test]
async fn a_shopper_changes_the_email_they_sign_in_with() -> anyhow::Result<()> {
    let (router, store, _) = shop().await?;
    let mut browser = Browser::new(&router);
    browser.post("/register", REGISTER).await;
    let form = text(browser.get("/account/email").await).await?;
    assert!(form.contains("ada@example.com"), "{form}");

    // The current password is asked again; a taken or unchanged address is refused.
    let wrong = browser
        .post(
            "/account/email",
            "email=ada.new%40example.com&password=nope",
        )
        .await;
    assert_eq!(wrong.status(), StatusCode::OK);
    assert!(text(wrong).await?.contains("Mot de passe incorrect"));
    let taken = browser
        .post(
            "/account/email",
            &format!(
                "email={}&password=analytical-engine",
                seed::SHOPPER_EMAIL.replace('@', "%40")
            ),
        )
        .await;
    assert!(text(taken).await?.contains("existe déjà"));
    let same = browser
        .post(
            "/account/email",
            "email=ADA%40example.com&password=analytical-engine",
        )
        .await;
    assert!(text(same).await?.contains("déjà l"));
    let invalid = browser
        .post(
            "/account/email",
            "email=not-an-email&password=analytical-engine",
        )
        .await;
    assert!(text(invalid).await?.contains("invalide"));

    let changed = browser
        .post(
            "/account/email",
            "email=Ada.New%40example.com&password=analytical-engine",
        )
        .await;
    assert_eq!(location(&changed), "/account");
    db::run_subscriptions_once(&store).await?;

    // Still signed in, under the new address — on the account and the customer.
    let account = text(browser.get("/account").await).await?;
    assert!(account.contains("ada.new@example.com"), "{account}");
    let customers = timada_customer::list_customers(
        &store.db,
        &timada_customer::ListCustomers {
            q: Some("ada.new@example.com".into()),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(customers.len(), 1);

    // The old address no longer signs in, the new one does, and the old one
    // was told about the change.
    let mut old = Browser::new(&router);
    let refused = old
        .post(
            "/login",
            "email=ada%40example.com&password=analytical-engine",
        )
        .await;
    assert_eq!(refused.status(), StatusCode::OK);
    let mut new = Browser::new(&router);
    let signed_in = new
        .post(
            "/login",
            "email=ada.new%40example.com&password=analytical-engine",
        )
        .await;
    assert_eq!(location(&signed_in), "/account");
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    let notice = outbox
        .iter()
        .find(|m| m.kind == "email-changed")
        .ok_or_else(|| anyhow::anyhow!("no notice: {outbox:?}"))?;
    assert_eq!(notice.recipient, "ada@example.com");
    assert!(
        notice.body.contains("ada.new@example.com"),
        "{}",
        notice.body
    );

    // The address left behind is free again for someone else.
    let mut newcomer = Browser::new(&router);
    let registered = newcomer.post("/register", REGISTER).await;
    assert_eq!(registered.status(), StatusCode::SEE_OTHER);
    Ok(())
}

#[tokio::test]
async fn a_cart_is_saved_for_later_reopened_and_deleted() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=2"))
        .await;

    // Saving is for signed-in shoppers.
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("pour sauvegarder ce panier"), "{cart}");
    let guest = browser.post("/cart/save", "name=Bureau").await;
    assert!(
        location(&guest).starts_with("/login"),
        "{}",
        location(&guest)
    );

    browser.post("/register", REGISTER).await;
    let saved = browser.post("/cart/save", "name=Bureau").await;
    assert_eq!(location(&saved), "/account/carts");
    db::run_subscriptions_once(&store).await?;
    let list = text(browser.get("/account/carts").await).await?;
    assert!(list.contains("Bureau"), "{list}");
    assert!(list.contains("239,90 €"), "{list}");
    // The browser starts over with an empty cart.
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("Votre panier est vide."), "{cart}");

    // Another shopper sees nothing, and cannot touch it.
    let cart_id = timada_cart::saved_carts_of_customer(
        &store.db,
        &timada_customer::list_customers(
            &store.db,
            &timada_customer::ListCustomers {
                q: Some("ada@example.com".into()),
                ..Default::default()
            },
        )
        .await?[0]
            .customer_id,
    )
    .await?[0]
        .cart_id
        .clone();
    let mut other = Browser::new(&router);
    other
        .post(
            "/login",
            &format!(
                "email={}&password={}",
                seed::SHOPPER_EMAIL.replace('@', "%40"),
                seed::SHOPPER_PASSWORD
            ),
        )
        .await;
    assert!(
        text(other.get("/account/carts").await)
            .await?
            .contains("Aucun panier sauvegardé.")
    );
    let stolen = other
        .post(&format!("/account/carts/{cart_id}/reopen"), "")
        .await;
    assert_eq!(stolen.status(), StatusCode::NOT_FOUND);

    // A current cart with articles is not silently abandoned.
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    let busy = browser
        .post(&format!("/account/carts/{cart_id}/reopen"), "")
        .await;
    assert_eq!(busy.status(), StatusCode::OK);
    assert!(text(busy).await?.contains("contient des articles"));
    browser
        .post(&format!("/cart/lines/{product_id}/remove"), "")
        .await;

    // Reopened: it is the current cart again and leaves the list.
    let reopened = browser
        .post(&format!("/account/carts/{cart_id}/reopen"), "")
        .await;
    assert_eq!(location(&reopened), "/cart");
    db::run_subscriptions_once(&store).await?;
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("239,90 €"), "{cart}");
    let list = text(browser.get("/account/carts").await).await?;
    assert!(list.contains("Aucun panier sauvegardé."), "{list}");

    // Saved again, then deleted.
    browser.post("/cart/save", "name=Plus+tard").await;
    db::run_subscriptions_once(&store).await?;
    let deleted = browser
        .post(&format!("/account/carts/{cart_id}/discard"), "")
        .await;
    assert_eq!(location(&deleted), "/account/carts");
    db::run_subscriptions_once(&store).await?;
    let list = text(browser.get("/account/carts").await).await?;
    assert!(list.contains("Aucun panier sauvegardé."), "{list}");
    Ok(())
}

#[tokio::test]
async fn a_shopper_changes_their_password() -> anyhow::Result<()> {
    let (router, store, _) = shop().await?;
    let mut browser = Browser::new(&router);
    browser.post("/register", REGISTER).await;
    // A second device signed in with the same account.
    let mut phone = Browser::new(&router);
    phone
        .post(
            "/login",
            "email=ada%40example.com&password=analytical-engine",
        )
        .await;
    assert_eq!(phone.get("/account").await.status(), StatusCode::OK);

    let wrong = browser
        .post(
            "/account/password",
            "current=nope&new=difference-engine&confirm=difference-engine",
        )
        .await;
    assert!(text(wrong).await?.contains("actuel incorrect"));
    let mismatch = browser
        .post(
            "/account/password",
            "current=analytical-engine&new=difference-engine&confirm=difference-engin",
        )
        .await;
    assert!(text(mismatch).await?.contains("pas identiques"));
    let weak = browser
        .post(
            "/account/password",
            "current=analytical-engine&new=short&confirm=short",
        )
        .await;
    assert!(text(weak).await?.contains("au moins 8"));

    let changed = browser
        .post(
            "/account/password",
            "current=analytical-engine&new=difference-engine&confirm=difference-engine",
        )
        .await;
    assert_eq!(location(&changed), "/account");

    // This browser stays signed in; the other device is signed out.
    assert_eq!(browser.get("/account").await.status(), StatusCode::OK);
    assert_eq!(phone.get("/account").await.status(), StatusCode::SEE_OTHER);
    // Only the new password signs in.
    let mut fresh = Browser::new(&router);
    let old = fresh
        .post(
            "/login",
            "email=ada%40example.com&password=analytical-engine",
        )
        .await;
    assert_eq!(old.status(), StatusCode::OK);
    let new = fresh
        .post(
            "/login",
            "email=ada%40example.com&password=difference-engine",
        )
        .await;
    assert_eq!(location(&new), "/account");
    // And the shopper is told.
    let outbox = timada_mailer::list_outbox(&store.db, None, 50, 0).await?;
    assert!(
        outbox
            .iter()
            .any(|m| m.kind == "password-changed" && m.recipient == "ada@example.com"),
        "{outbox:?}"
    );
    Ok(())
}

#[tokio::test]
async fn the_checkout_prices_and_delivers_for_the_address_zone() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=2"))
        .await;
    browser.post("/register", REGISTER).await;
    // Toulouse, Le Marin (Martinique) and New York.
    browser.post("/account/addresses/new", ADDRESS).await;
    let overseas = ADDRESS
        .replace("postal_code=31000", "postal_code=97290")
        .replace("city=Toulouse", "city=Le+Marin")
        .replace("country_code=fr", "country_code=mq");
    browser.post("/account/addresses/new", &overseas).await;
    let abroad = ADDRESS
        .replace("postal_code=31000", "postal_code=10001")
        .replace("city=Toulouse", "city=New+York")
        .replace("country_code=fr", "country_code=us");
    browser.post("/account/addresses/new", &abroad).await;

    let page = text(browser.get("/checkout").await).await?;
    let address_of = |city: &str| -> anyhow::Result<String> {
        page.split("name=\"address\" value=\"")
            .skip(1)
            .find(|chunk| chunk.contains(city))
            .and_then(|chunk| chunk.split('"').next())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("no address radio for {city}"))
    };
    let (metro, marin, new_york) = (
        address_of("Toulouse")?,
        address_of("Le Marin")?,
        address_of("New York")?,
    );

    // Metropolitan France: listed prices, home delivery and pickup.
    let page = text(browser.get(&format!("/checkout?address={metro}")).await).await?;
    assert!(page.contains("119,95 €"), "{page}");
    assert!(page.contains("Colissimo"), "{page}");
    assert!(!page.contains("Chronopost"), "{page}");
    assert!(!page.contains("hors TVA"), "{page}");

    // Martinique: an export. Prices without French VAT, its own carrier.
    let page = text(browser.get(&format!("/checkout?address={marin}")).await).await?;
    assert!(page.contains("vente hors TVA française"), "{page}");
    assert!(page.contains("99,96 €"), "{page}");
    assert!(page.contains("199,92 €"), "{page}");
    assert!(page.contains("Chronopost (DOM-TOM) — 19,96 €"), "{page}");
    assert!(!page.contains("Colissimo"), "{page}");

    // The United States are in no zone: nothing to submit.
    let page = text(browser.get(&format!("/checkout?address={new_york}")).await).await?;
    assert!(
        page.contains("Nous ne livrons pas encore ce pays"),
        "{page}"
    );
    assert!(!page.contains("Valider la commande"), "{page}");
    let forced = browser
        .post(
            "/checkout",
            &format!("delivery_address_id={new_york}&delivery_method=colissimo&payment_mode=card"),
        )
        .await;
    assert_eq!(forced.status(), StatusCode::OK);
    assert!(
        text(forced)
            .await?
            .contains("Nous ne livrons pas encore ce pays")
    );
    // Nor can a method of another zone be forced onto Martinique.
    let wrong_carrier = browser
        .post(
            "/checkout",
            &format!("delivery_address_id={marin}&delivery_method=colissimo&payment_mode=card"),
        )
        .await;
    let refused = text(wrong_carrier).await?;
    assert!(
        refused.contains("ne dessert pas cette adresse"),
        "{refused}"
    );
    assert!(refused.contains("vente hors TVA française"), "{refused}");

    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={marin}&delivery_method=chronopost-dom&payment_mode=card"
            ),
        )
        .await;
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;

    // Charged what the checkout showed: 199,92 + 19,96, no VAT.
    let detail = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(detail.contains("Total HT"), "{detail}");
    assert!(detail.contains("219,88 €"), "{detail}");
    assert!(detail.contains("Exonération de TVA"), "{detail}");
    let payment =
        timada_payment::load_payment(&store.executor, timada_payment::payment_id(&order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("payment not requested"))?;
    assert_eq!(payment.amount, timada_core::Money::eur(21_988));
    let confirmation = timada_mailer::list_outbox(&store.db, None, 50, 0)
        .await?
        .into_iter()
        .find(|m| m.kind == "order-confirmation" && m.recipient == "ada@example.com")
        .ok_or_else(|| anyhow::anyhow!("no confirmation e-mail"))?;
    assert!(
        confirmation.body.contains("Total HT — 219,88 €"),
        "{}",
        confirmation.body
    );
    Ok(())
}

#[tokio::test]
async fn an_eu_delivery_is_priced_with_the_vat_of_its_country() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=2"))
        .await;
    browser.post("/register", REGISTER).await;
    let berlin = ADDRESS
        .replace("postal_code=31000", "postal_code=10117")
        .replace("city=Toulouse", "city=Berlin")
        .replace("country_code=fr", "country_code=de");
    browser.post("/account/addresses/new", &berlin).await;

    // 119,95 TTC is 99,96 HT; with 19 % of German VAT, 118,95. Its own
    // carrier, taxed the same way: 12,90 TTC → 10,75 HT → 12,79.
    let page = text(browser.get("/checkout").await).await?;
    assert!(page.contains("TVA du pays de livraison"), "{page}");
    assert!(page.contains("Allemagne"), "{page}");
    assert!(page.contains("118,95 €"), "{page}");
    assert!(page.contains("237,90 €"), "{page}");
    assert!(page.contains("Colissimo Europe — 12,79 €"), "{page}");
    assert!(!page.contains("Colissimo à domicile"), "{page}");
    assert!(!page.contains("hors TVA"), "{page}");
    let address_id = page
        .split("name=\"address\" value=\"")
        .nth(1)
        .and_then(|chunk| chunk.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no address radio"))?
        .to_owned();

    // The metropolitan carrier does not go there.
    let wrong_carrier = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    assert!(
        text(wrong_carrier)
            .await?
            .contains("ne dessert pas cette adresse")
    );

    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo-europe&payment_mode=card"
            ),
        )
        .await;
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;

    // Charged what the checkout showed, German VAT inside.
    let order = timada_order::load_order_details(&store.executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.total, timada_core::Money::eur(25_069));
    let tax = order
        .tax
        .ok_or_else(|| anyhow::anyhow!("order not taxed"))?;
    assert_eq!(tax.zone_code, "de");
    assert_eq!(tax.treatment, timada_tax::TaxTreatment::DestinationVat);
    assert_eq!(tax.vat_lines.len(), 1);
    assert_eq!(tax.vat_lines[0].rate_bp, 1_900);

    let detail = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(detail.contains("Total TTC"), "{detail}");
    assert!(detail.contains("250,69 €"), "{detail}");
    assert!(detail.contains("dont TVA 19"), "{detail}");
    assert!(detail.contains("40,03 €"), "{detail}");
    assert!(detail.contains("État membre de livraison"), "{detail}");
    assert!(!detail.contains("Exonération"), "{detail}");
    let payment =
        timada_payment::load_payment(&store.executor, timada_payment::payment_id(&order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("payment not requested"))?;
    assert_eq!(payment.amount, timada_core::Money::eur(25_069));
    Ok(())
}

#[tokio::test]
async fn a_cart_is_checked_out_at_todays_price() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    // Parked among the saved carts at 119,95 €.
    browser.post("/cart/save", "name=Plus+tard").await;
    db::run_subscriptions_once(&store).await?;
    let customer = timada_customer::list_customers(
        &store.db,
        &timada_customer::ListCustomers {
            q: Some("ada@example.com".into()),
            ..Default::default()
        },
    )
    .await?;
    let cart_id = timada_cart::saved_carts_of_customer(&store.db, &customer[0].customer_id).await?
        [0]
    .cart_id
    .clone();

    // The price goes up while it waits; the shopper takes the cart up again.
    let pricing = timada_pricing::Command(&store.executor);
    pricing
        .change_price(
            timada_pricing::price_id(&product_id),
            timada_core::Money::eur(12_995),
        )
        .await?;
    browser
        .post(&format!("/account/carts/{cart_id}/reopen"), "")
        .await;
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("est passé de 119,95 € à 129,95 €"), "{cart}");
    assert!(cart.contains("129,95 €"), "{cart}");
    // Told once: the cart is up to date now.
    let cart = text(browser.get("/cart").await).await?;
    assert!(!cart.contains("est passé de"), "{cart}");

    // The price moves again between the checkout page and its submission:
    // the order is not placed at a total the shopper has not seen.
    let checkout = text(browser.get("/checkout").await).await?;
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address"))?
        .to_owned();
    pricing
        .change_price(
            timada_pricing::price_id(&product_id),
            timada_core::Money::eur(13_995),
        )
        .await?;
    let form =
        format!("delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card");
    let held = browser.post("/checkout", &form).await;
    assert_eq!(held.status(), StatusCode::OK);
    let held = text(held).await?;
    assert!(held.contains("Vérifiez le nouveau total"), "{held}");
    assert!(held.contains("139,95 €"), "{held}");

    // Seen, confirmed, and charged at that price.
    let placed = browser.post("/checkout", &form).await;
    assert_eq!(placed.status(), StatusCode::SEE_OTHER);
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;
    let order = timada_order::load_order_details(&store.executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order not placed"))?;
    assert_eq!(order.subtotal, timada_core::Money::eur(13_995));

    // A product no longer sold leaves the cart, with a word about it.
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    pricing
        .withdraw_price(timada_pricing::price_id(&product_id))
        .await?;
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("plus en vente"), "{cart}");
    assert!(cart.contains("Votre panier est vide."), "{cart}");
    Ok(())
}

#[tokio::test]
async fn reviews_and_questions_are_paged_separately() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let reviews = timada_review::Command(&store.executor);
    for index in 1..=7 {
        let id = reviews
            .submit_review(timada_review::SubmitReview {
                product_id: product_id.clone(),
                customer_id: format!("customer-{index}"),
                order_id: None,
                rating: 4,
                title: String::new(),
                body: format!("Avis numéro {index}."),
            })
            .await?;
        reviews.publish_review(&id).await?;
        // Same-millisecond events have no order: space them.
        tokio::time::sleep(std::time::Duration::from_millis(1_050)).await;
    }
    let question = reviews
        .ask_question(timada_review::AskQuestion {
            product_id: product_id.clone(),
            customer_id: "customer-1".into(),
            body: "Compatible G-SYNC ?".into(),
        })
        .await?;
    reviews
        .answer_question(&question, timada_review::AnswerAuthor::Staff, "Oui.".into())
        .await?;
    db::run_subscriptions_once(&store).await?;

    let mut browser = Browser::new(&router);
    let product = format!("/p/{product_id}");
    // Newest first, five a page.
    let first = text(browser.get(&product).await).await?;
    assert!(first.contains("Avis numéro 7."), "{first}");
    assert!(first.contains("Avis numéro 3."), "{first}");
    assert!(!first.contains("Avis numéro 2."), "{first}");
    assert!(first.contains("Avis — page 1 sur 2"), "{first}");
    assert!(first.contains(&format!("{product}?avis=2#avis")), "{first}");
    // One page of questions: no pager for them.
    assert!(!first.contains("Questions — page"), "{first}");

    let second = text(browser.get(&format!("{product}?avis=2")).await).await?;
    assert!(second.contains("Avis numéro 2."), "{second}");
    assert!(second.contains("Avis numéro 1."), "{second}");
    assert!(!second.contains("Avis numéro 3."), "{second}");
    assert!(
        second.contains(&format!("{product}?avis=1#avis")),
        "{second}"
    );
    // The questions stay where they were.
    assert!(second.contains("Compatible G-SYNC ?"), "{second}");
    // A page past the end is the last one.
    let beyond = text(browser.get(&format!("{product}?avis=9")).await).await?;
    assert!(beyond.contains("Avis — page 2 sur 2"), "{beyond}");
    Ok(())
}

#[tokio::test]
async fn a_shopper_picks_a_currency_and_is_shown_and_charged_in_it() -> anyhow::Result<()> {
    let (router, store, product_id) = shop().await?;
    let mut browser = Browser::new(&router);
    let product_uri = format!("/p/{product_id}");

    // Euros until told otherwise; the header offers the shop's currencies.
    let page = text(browser.get(&product_uri).await).await?;
    assert!(page.contains("119,95 €"), "{page}");
    assert!(page.contains("ou 3 × "), "{page}");
    assert!(page.contains("name=\"currency\""), "{page}");
    assert!(page.contains("GBP (£)"), "{page}");

    // A currency the shop does not sell in is not a choice.
    let unknown = browser.post("/currency", "currency=USD&next=%2F").await;
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    // Pounds: back where the shopper was, and everything follows — the
    // product page, the listing, its price filter.
    let switched = browser
        .post(
            "/currency",
            &format!("currency=GBP&next=%2Fp%2F{product_id}"),
        )
        .await;
    assert_eq!(location(&switched), product_uri);
    let page = text(browser.get(&product_uri).await).await?;
    assert!(page.contains("109,00 £"), "{page}");
    assert!(!page.contains("119,95"), "{page}");
    // The instalment offer is a euro one.
    assert!(!page.contains("ou 3 × "), "{page}");
    let listing = text(browser.get("/recherche?q=AOC").await).await?;
    assert!(listing.contains("109,00 £"), "{listing}");
    let dearer = text(browser.get("/recherche?q=AOC&prix_min=115").await).await?;
    assert!(!dearer.contains("109,00 £"), "{dearer}");

    // Into the cart at its pound price.
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("109,00 £"), "{cart}");

    // A cart never mixes currencies: going back to euros is asked, not done.
    let asked = browser.post("/currency", "currency=EUR&next=%2Fcart").await;
    assert_eq!(location(&asked), "/cart?devise=EUR&next=%2Fcart");
    let confirm = text(browser.get(&location(&asked)).await).await?;
    assert!(
        confirm.contains("Vider le panier et passer en €"),
        "{confirm}"
    );
    assert!(confirm.contains("Garder mon panier"), "{confirm}");
    // Kept: the shop stays in pounds, the cart's currency.
    let page = text(browser.get(&product_uri).await).await?;
    assert!(page.contains("109,00 £"), "{page}");

    // Checked out in pounds: delivery and instalments at their pound fees,
    // a pound order and payment.
    browser.post("/register", REGISTER).await;
    browser.post("/account/addresses/new", ADDRESS).await;
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("4,90 £"), "{checkout}");
    // Not a euro on the page itself (the header's switcher names them).
    let body = checkout.split("<main>").nth(1).unwrap_or_default();
    assert!(!body.contains('€'), "{body}");
    // Paying in several times has its pound fee too.
    assert!(checkout.contains("value=\"installments\""), "{checkout}");
    assert!(checkout.contains("3,99 £"), "{checkout}");
    let address_id = checkout
        .split("name=\"delivery_address_id\" value=\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .ok_or_else(|| anyhow::anyhow!("no delivery address"))?
        .to_owned();
    let placed = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=card"
            ),
        )
        .await;
    let order_id = location(&placed)
        .rsplit('/')
        .next()
        .unwrap_or_default()
        .to_owned();
    db::run_subscriptions_once(&store).await?;
    let order = timada_order::load_order_details(&store.executor, &order_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("order missing"))?;
    assert_eq!(order.shipping_fee, timada_core::Money::new(490, "GBP"));
    assert_eq!(order.total, timada_core::Money::new(11_390, "GBP"));
    let payment =
        timada_payment::load_payment(&store.executor, timada_payment::payment_id(&order_id))
            .await?
            .ok_or_else(|| anyhow::anyhow!("payment missing"))?;
    assert_eq!(payment.amount, timada_core::Money::new(11_390, "GBP"));
    let order_page = text(browser.get(&format!("/account/orders/{order_id}")).await).await?;
    assert!(order_page.contains("113,90 £"), "{order_page}");

    // The cart is gone: euros again at once. Then a euro cart, emptied on
    // purpose to shop in pounds.
    let back = browser
        .post(
            "/currency",
            &format!("currency=EUR&next=%2Fp%2F{product_id}"),
        )
        .await;
    assert_eq!(location(&back), product_uri);
    assert!(
        text(browser.get(&product_uri).await)
            .await?
            .contains("119,95 €")
    );
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    let emptied = browser
        .post("/currency", "currency=GBP&next=%2Fcart&empty_cart=on")
        .await;
    assert_eq!(location(&emptied), "/cart");
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("Votre panier est vide."), "{cart}");
    assert!(
        text(browser.get(&product_uri).await)
            .await?
            .contains("109,00 £")
    );
    // An emptied cart is in no currency: it takes a pound line at once.
    browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    let cart = text(browser.get("/cart").await).await?;
    assert!(cart.contains("109,00 £"), "{cart}");
    browser
        .post(&format!("/cart/lines/{product_id}/remove"), "")
        .await;

    // Francs: the shop said nothing of what instalments cost there, so they
    // are not offered — and not taken if asked for anyway.
    browser.post("/currency", "currency=CHF&next=%2F").await;
    let added = browser
        .post("/cart/add", &format!("product_id={product_id}&quantity=1"))
        .await;
    assert_eq!(location(&added), "/cart", "{:?}", text(added).await);
    let checkout = text(browser.get("/checkout").await).await?;
    assert!(checkout.contains("129,00 CHF"), "{checkout}");
    assert!(!checkout.contains("value=\"installments\""), "{checkout}");
    let sneaky = browser
        .post(
            "/checkout",
            &format!(
                "delivery_address_id={address_id}&delivery_method=colissimo&payment_mode=installments"
            ),
        )
        .await;
    assert!(text(sneaky).await?.contains("n'est pas proposé"));
    Ok(())
}
