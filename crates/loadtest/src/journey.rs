//! One shopper's visit, drawn at random from what shoppers do: land on the
//! home page, a category, a product (from a search engine) or a search;
//! wander a few pages, thinking between them; now and then put something in
//! the cart, change it, try the promo code; sometimes check out, and mostly
//! pay. Every choice comes from the visit's own seed, so a seed replays the
//! same visit.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use rand::rngs::StdRng;
use rand::seq::IndexedRandom;
use rand::{Rng, SeedableRng};
use serde::Deserialize;

use crate::http::{Http, Visitor};
use crate::site::{self, Catalog};
use crate::stripe::FakeStripe;

/// How long a paid order may take to show before it counts as lost.
const ORDER_DEADLINE: Duration = Duration::from_secs(60);
/// The promo code `seed` gives the shop.
const PROMO_CODE: &str = "MONSTER10";

/// How shoppers behave, as probabilities and means.
#[derive(Clone, Debug)]
pub struct Mix {
    /// Pages a visit reads after landing, on average.
    pub pages: f64,
    /// Seconds between two pages, on average (exponentially spread).
    pub think_secs: f64,
    /// That a product page read ends in the cart.
    pub add_to_cart: f64,
    /// That a visit with a cart goes to the checkout.
    pub checkout: f64,
    /// That a shopper at the checkout pays.
    pub pay: f64,
}

pub struct Context {
    pub http: Http,
    pub stripe: Arc<FakeStripe>,
    pub catalog: Catalog,
    pub mix: Mix,
}

pub async fn visit(context: &Context, seed: u64) {
    let metrics = context.http.metrics();
    metrics.count("visit.started");
    let mut shopper = Shopper {
        context,
        visitor: Visitor::new(&context.http),
        rng: StdRng::seed_from_u64(seed),
        in_cart: 0,
    };
    match shopper.walk().await {
        Ok(()) => metrics.count("visit.finished"),
        // The request that failed is already counted under its route.
        Err(err) => {
            tracing::debug!(seed, "visit cut short: {err:#}");
            metrics.count("visit.failed");
        }
    }
}

struct Shopper<'a> {
    context: &'a Context,
    visitor: Visitor<'a>,
    rng: StdRng,
    in_cart: u32,
}

impl Shopper<'_> {
    async fn walk(&mut self) -> Result<()> {
        match self.rng.random_range(0..100) {
            0..40 => drop(self.visitor.get("/").await?),
            40..75 => self.category().await?,
            75..90 => self.product().await?,
            _ => self.search().await?,
        }
        // Geometric, so most visits are short and a few are long.
        let more = 1.0 / (1.0 + self.context.mix.pages);
        while !self.rng.random_bool(more) {
            self.think().await;
            match self.rng.random_range(0..100) {
                0..45 => self.product().await?,
                45..70 => self.category().await?,
                70..85 => self.search().await?,
                85..95 => self.boutique().await?,
                _ => {
                    if self.in_cart > 0 {
                        drop(self.visitor.get("/cart").await?);
                    }
                }
            }
        }
        if self.in_cart == 0 {
            return Ok(());
        }
        self.think().await;
        self.tend_cart().await?;
        if self.in_cart > 0 && self.rng.random_bool(self.context.mix.checkout) {
            self.think().await;
            self.checkout().await?;
        }
        Ok(())
    }

    async fn think(&mut self) {
        let mean = self.context.mix.think_secs;
        if mean <= 0.0 {
            return;
        }
        // Exponential, capped: shoppers rarely stare at a page for minutes.
        let secs = (-mean * (1.0 - self.rng.random::<f64>()).ln()).min(mean * 10.0);
        tokio::time::sleep(Duration::from_secs_f64(secs)).await;
    }

    async fn category(&mut self) -> Result<()> {
        let Some(slug) = self
            .context
            .catalog
            .categories
            .choose(&mut self.rng)
            .cloned()
        else {
            return Ok(());
        };
        let mut path = format!("/c/{slug}");
        match self.rng.random_range(0..100) {
            0..70 => {}
            70..85 => path.push_str("?page=2"),
            85..93 => path.push_str("?sort=price"),
            _ => path.push_str("?min_price=10&max_price=40"),
        }
        self.visitor.get(&path).await?;
        Ok(())
    }

    async fn product(&mut self) -> Result<()> {
        let Some(slug) = self.context.catalog.products.choose(&mut self.rng).cloned() else {
            return Ok(());
        };
        let page = self.visitor.get(&format!("/p/{slug}")).await?;
        if self.rng.random_bool(self.context.mix.add_to_cart) {
            self.think().await;
            self.add(&slug, &page.body).await?;
        }
        Ok(())
    }

    async fn add(&mut self, slug: &str, body: &str) -> Result<()> {
        let metrics = self.context.http.metrics();
        if site::product_sold_out(body) {
            metrics.count("cart.sold_out");
            return Ok(());
        }
        let quantity = match self.rng.random_range(0..100) {
            0..85 => 1,
            85..97 => 2,
            _ => 3,
        };
        let mut form = vec![
            ("slug", slug.to_owned()),
            ("quantity", quantity.to_string()),
            ("from", format!("/p/{slug}")),
        ];
        let variants = site::variants_on_sale(body);
        if body.contains("data-variants") {
            let Some(variant) = variants.choose(&mut self.rng) else {
                metrics.count("cart.sold_out");
                return Ok(());
            };
            form.push(("variant_id", variant.clone()));
        }
        let page = self.visitor.post("/cart/add", &form).await?;
        match site::error_of(&page.path) {
            Some(code) => metrics.count(&format!("cart.refused.{code}")),
            None => {
                metrics.count("cart.added");
                self.in_cart += quantity;
            }
        }
        Ok(())
    }

    /// Typing in the search box: the dropdown asks again as letters come.
    async fn search(&mut self) -> Result<()> {
        let Some(term) = self.context.catalog.terms.choose(&mut self.rng).cloned() else {
            return Ok(());
        };
        let typed = self.rng.random_range(2..=term.len().min(6));
        for end in 2..=typed {
            let Some(prefix) = term.get(..end) else { break };
            self.visitor
                .get(&format!("/search?q={}", crate::http::encode(prefix)))
                .await?;
            tokio::time::sleep(Duration::from_millis(self.rng.random_range(80..250))).await;
        }
        if self.rng.random_bool(0.4) {
            self.visitor
                .get(&format!("/boutique?q={}", crate::http::encode(&term)))
                .await?;
        }
        Ok(())
    }

    async fn boutique(&mut self) -> Result<()> {
        let path = match self.rng.random_range(0..3) {
            0 => "/boutique".to_owned(),
            1 => "/boutique?page=2".to_owned(),
            _ => "/boutique?min_price=5&max_price=25".to_owned(),
        };
        self.visitor.get(&path).await?;
        Ok(())
    }

    /// The cart page, with a change of mind now and then.
    async fn tend_cart(&mut self) -> Result<()> {
        let metrics = self.context.http.metrics();
        let page = self.visitor.get("/cart").await?;
        let lines = site::cart_lines(&page.body);
        if lines.is_empty() {
            metrics.anomaly(
                "cart_lost",
                format!("{} added, the cart page shows none", self.in_cart),
            );
            self.in_cart = 0;
            return Ok(());
        }
        if self.rng.random_bool(0.15) {
            let line = lines.choose(&mut self.rng).expect("not empty");
            let quantity = if self.rng.random_bool(0.3) {
                0
            } else {
                line.quantity + 1
            };
            self.visitor
                .post(
                    "/cart/quantity",
                    &[
                        ("product_id", line.product_id.clone()),
                        ("variant_id", line.variant_id.clone()),
                        ("quantity", quantity.to_string()),
                        ("back", String::new()),
                        ("from", "/cart".into()),
                    ],
                )
                .await?;
            metrics.count("cart.changed");
            if quantity == 0 && lines.len() == 1 {
                self.in_cart = 0;
            }
        }
        if self.in_cart > 0 && self.rng.random_bool(0.12) {
            let page = self
                .visitor
                .post(
                    "/cart/discount",
                    &[
                        ("code", PROMO_CODE.into()),
                        ("back", String::new()),
                        ("from", "/cart".into()),
                    ],
                )
                .await?;
            metrics.count(match site::error_of(&page.path) {
                Some(_) => "cart.code_refused",
                None => "cart.code_applied",
            });
        }
        Ok(())
    }

    async fn checkout(&mut self) -> Result<()> {
        let metrics = self.context.http.metrics();
        let page = self.visitor.get("/checkout").await?;
        if !page.path.starts_with("/checkout") {
            metrics.count("checkout.sent_back");
            return Ok(());
        }
        let form = site::checkout_form(&page.body).context("the checkout page has no form")?;
        let country = match self.rng.random_range(0..100) {
            0..80 => "FR",
            80..92 => "BE",
            _ => "CH",
        };
        let method = form.methods.iter().find(|(_, _, countries)| {
            countries.is_empty() || countries.iter().any(|c| c == country)
        });
        let expected = form.goods_cents + method.map_or(0, |(_, amount, _)| *amount);
        self.think().await;
        let n = self.rng.random_range(0..1_000_000u32);
        let mut fields = vec![
            ("expected_total_cents", expected.to_string()),
            ("email", format!("shopper{n}@example.org")),
            ("name", format!("Shopper {n}")),
            ("phone", String::new()),
            ("line1", format!("{} rue de la Paix", n % 200 + 1)),
            ("line2", String::new()),
            ("city", "Paris".into()),
            ("postal_code", "75002".into()),
            ("country", country.into()),
            ("note", String::new()),
        ];
        if let Some((id, _, _)) = method {
            fields.push(("shipping_method", id.clone()));
        }
        if self.rng.random_bool(0.2) {
            fields.push(("newsletter", "1".into()));
        }
        metrics.count("checkout.asked");
        let started = self.visitor.post("/checkout/start", &fields).await?;
        let started: Started = serde_json::from_str(&started.body)
            .with_context(|| format!("/checkout/start answered {:?}", started.body))?;
        let Some(client_secret) = started.client_secret else {
            let why = started
                .redirect
                .as_deref()
                .and_then(site::error_of)
                .map(str::to_owned)
                .or(started
                    .error
                    .as_deref()
                    .map(|text| code_of(text).to_owned()))
                .unwrap_or_else(|| "unknown".into());
            metrics.count(&format!("checkout.refused.{why}"));
            if why == "invoicing_not_configured" || why == "other" {
                metrics.anomaly("checkout_refused", format!("{why}: {:?}", started.error));
            }
            return Ok(());
        };
        metrics.count("checkout.opened");
        match self.context.stripe.amount_of(&client_secret) {
            Some(amount) if amount == expected => {}
            amount => metrics.anomaly(
                "amount_mismatch",
                format!("the page showed {expected}, Stripe was asked for {amount:?}"),
            ),
        }
        if !self.rng.random_bool(self.context.mix.pay) {
            metrics.count("checkout.abandoned");
            return Ok(());
        }
        // Filling in the card.
        self.think().await;
        if !self.context.stripe.pay(&client_secret) {
            metrics.anomaly("unknown_payment", client_secret.clone());
            return Ok(());
        }
        metrics.count("payment.paid");
        self.await_order(&client_secret).await
    }

    /// Back from Stripe: the page waits for the dashboard, reloading every
    /// two seconds as its script does, until it sends on to the order.
    async fn await_order(&mut self, client_secret: &str) -> Result<()> {
        let metrics = self.context.http.metrics();
        let paid = Instant::now();
        let back = format!(
            "/checkout/complete?payment_intent_client_secret={}&redirect_status=succeeded",
            crate::http::encode(client_secret)
        );
        loop {
            let page = self.visitor.get(&back).await?;
            if page.path.starts_with("/orders/") {
                metrics.order_latency(paid.elapsed());
                if !page.body.contains(r#"data-order-status="paid""#) {
                    metrics.anomaly("order_not_paid", page.path.clone());
                }
                metrics.count("order.shown");
                return Ok(());
            }
            if !page.path.starts_with("/checkout/complete") {
                metrics.anomaly("payment_bounced", format!("sent to {}", page.path));
                return Ok(());
            }
            if paid.elapsed() > ORDER_DEADLINE {
                metrics.anomaly(
                    "order_missing",
                    format!("{client_secret}: no order {ORDER_DEADLINE:?} after paying"),
                );
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

/// The refusal a `/checkout/start` answer words for the shopper, as the
/// shop's code names it (examples/chatounette/src/app/mod.rs, `say`).
fn code_of(text: &str) -> &'static str {
    const CODES: &[(&str, &str)] = &[
        ("Le paiement est indisponible", "payments_unavailable"),
        (
            "Cette boutique ne peut pas encore",
            "invoicing_not_configured",
        ),
        ("il n'en reste pas assez", "out_of_stock"),
        ("n'est plus en vente", "product_unavailable"),
        ("Ce code promo", "discount_refused"),
        ("Ce panier a déjà été commandé", "cart_checked_out"),
    ];
    CODES
        .iter()
        .find(|(words, _)| text.contains(words))
        .map_or("other", |(_, code)| code)
}

#[derive(Deserialize)]
struct Started {
    client_secret: Option<String>,
    error: Option<String>,
    redirect: Option<String>,
}
