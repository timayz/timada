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
    let (executor, pool) = timada_core::testing::memory_executor(db::migrations()).await?;
    let store = Store { executor, db: pool };
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
    assert!(confirmation.starts_with("/checkout/confirmation/"));
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
async fn questions_show_on_the_product_page_once_answered() -> anyhow::Result<()> {
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

    // Unanswered: the asker sees it waiting, other visitors do not see it.
    let own = text(browser.get(&product).await).await?;
    assert!(own.contains("en attente de réponse"), "{own}");
    assert!(own.contains("Compatible G-SYNC ?"), "{own}");
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(!public.contains("Compatible G-SYNC ?"), "{public}");

    // The shop answers: everyone sees the question and its answer.
    let rows =
        timada_review::list_questions(&store.db, &timada_review::ListQuestions::default()).await?;
    assert_eq!(rows.len(), 1);
    timada_review::Command(&store.executor)
        .answer_question(
            &rows[0].question_id,
            timada_review::AnswerAuthor::Staff,
            "Oui, G-SYNC Compatible.".into(),
        )
        .await?;
    db::run_subscriptions_once(&store).await?;
    let public = text(Browser::new(&router).get(&product).await).await?;
    assert!(public.contains("Compatible G-SYNC ?"), "{public}");
    assert!(public.contains("Réponse de la boutique"), "{public}");
    assert!(public.contains("Oui, G-SYNC Compatible."), "{public}");
    let own = text(browser.get(&product).await).await?;
    assert!(!own.contains("en attente de réponse"), "{own}");
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
