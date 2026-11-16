//! Tells Meta of a shop's sales from the server (its Conversions API), so a
//! sale is counted even when the browser's pixel event is lost (a blocker,
//! a tab closed before the order's page).
//!
//! A sale is told when three things hold: the shop set a pixel and its
//! Conversions API token (`timada-pages`' `MetaPixel`, the token sealed
//! under `secrets.key`), and the order has the shopper's
//! [`Browser`](timada_orders::Browser), which the storefront gives only for
//! a shopper who allowed advertising. The event carries the order's id as
//! its event id, the same the storefront's pixel sends with its `Purchase`,
//! so Meta counts the two once.
//!
//! The call is made inside the pass, as the VIES check is: this
//! subscription does nothing else, so a slow answer delays the other shops'
//! sales to Meta and nothing more, and each call takes at most
//! [`META_TIMEOUT`]. What Meta answers decides what follows:
//!
//! - taken: done;
//! - refused (the token was revoked, the pixel is not the token's): the
//!   shop's own fault, which asking again would meet again. Its reason goes
//!   on the pixel's stream, where the merchant reads it beside the token,
//!   and the pass goes on;
//! - no answer, or Meta's own failure: the pass fails and is tried again,
//!   for as long as the order is younger than [`GIVE_UP_AFTER_SECS`]. Past
//!   that the sale is left to the browser's event: one pixel Meta keeps
//!   failing on must not hold every other shop's sales for ever.
//!
//! Telling twice is harmless (Meta drops the repeat by the event id), so
//! every node of the region may run it and a pass may be replayed. A sale
//! older than [`MAX_EVENT_AGE_SECS`] is not sent: Meta refuses it.

use std::sync::Arc;
use std::time::Duration;

use evento::{
    Evento, Executor,
    metadata::Event,
    subscription::{Context, SubscriptionBuilder},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use timada_orders::aggregator::BrowserSet;
use timada_orders::{Browser, OrderState};

use crate::seal::Sealer;

/// The cursor identity of the subscription, in the store.
pub const SUBSCRIPTION: &str = "timada-dashboard/meta-conversions";
/// Who the pixel's stream says recorded a refusal.
pub const ISSUER: &str = "timada-dashboard";

/// Meta's Graph API, at the version the events are sent to.
pub const META_API_URL: &str = "https://graph.facebook.com/v26.0";

/// How long one call may take, connection and answer together.
pub const META_TIMEOUT: Duration = Duration::from_secs(5);

/// How old a sale may be for Meta to take it: seven days, less a margin.
pub const MAX_EVENT_AGE_SECS: u64 = 6 * 24 * 60 * 60;

/// How long a sale Meta does not answer about is tried again for.
pub const GIVE_UP_AFTER_SECS: u64 = 60 * 60;

/// What Meta says of an event sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaAnswer {
    Taken,
    /// Refused for what the shop set up, in Meta's words.
    Refused(String),
    /// Meta could not answer, or failed on its side.
    Unavailable(String),
}

#[async_trait::async_trait]
pub trait Meta: Send + Sync {
    /// Sends one server event to the pixel `pixel_id` under `token`.
    async fn send(&self, pixel_id: &str, token: &str, event: &Value) -> MetaAnswer;
}

pub struct MetaClient {
    url: String,
    client: reqwest::Client,
}

impl MetaClient {
    /// `url`: Meta's own ([`META_API_URL`]) when `None`.
    pub fn new(url: Option<String>) -> Self {
        Self {
            url: url
                .unwrap_or_else(|| META_API_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            client: reqwest::Client::builder()
                .timeout(META_TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }
}

/// Meta's words for a refusal, from the body of its answer.
fn refusal(body: &[u8], status: u16) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|answer| {
            let error = answer.get("error")?;
            let message = error.get("message")?.as_str()?.to_owned();
            Some(match error.get("code").and_then(Value::as_i64) {
                Some(code) => format!("{message} (code {code})"),
                None => message,
            })
        })
        .unwrap_or_else(|| format!("HTTP {status}"))
}

#[async_trait::async_trait]
impl Meta for MetaClient {
    async fn send(&self, pixel_id: &str, token: &str, event: &Value) -> MetaAnswer {
        // The token goes in the body, not in the address: addresses are
        // what proxies and logs keep.
        let body = json!({ "data": [event], "access_token": token });
        let response = self
            .client
            .post(format!("{}/{pixel_id}/events", self.url))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string())
            .send()
            .await;
        let response = match response {
            Ok(response) => response,
            // The error names the address, never the body.
            Err(err) => return MetaAnswer::Unavailable(err.without_url().to_string()),
        };
        let status = response.status();
        if status.is_success() {
            return MetaAnswer::Taken;
        }
        let body = response.bytes().await.unwrap_or_default();
        // Asked too often, or Meta's own failure: later may do. Anything
        // else it will say again.
        if status.as_u16() == 429 || status.is_server_error() {
            MetaAnswer::Unavailable(refusal(&body, status.as_u16()))
        } else {
            MetaAnswer::Refused(refusal(&body, status.as_u16()))
        }
    }
}

/// What tells Meta: the sealer of the shops' tokens (`None` without
/// `secrets.key`: no token could be set either) and the way to Meta.
pub struct Conversions {
    sealer: Option<Sealer>,
    meta: Arc<dyn Meta>,
}

impl Conversions {
    pub fn new(sealer: Option<Sealer>, meta: Arc<dyn Meta>) -> Self {
        Self { sealer, meta }
    }
}

/// The store, to read the pixel and the order from and to write a refusal
/// to.
#[derive(Clone)]
struct TenantStore(Evento);

/// Follows every tenant's orders whose shopper allowed advertising.
pub fn subscription<E: Executor>(
    conversions: Arc<Conversions>,
    store: Evento,
) -> SubscriptionBuilder<E> {
    SubscriptionBuilder::new(SUBSCRIPTION)
        .data(conversions)
        .data(TenantStore(store))
        .handler(on_browser_set())
        // A saga: it follows the node's own region (the executor's default
        // routing key), so a region's nodes do its work and the other
        // regions' nodes leave it to them.
        .retry(5)
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A name or a city as Meta hashes it: lowercase, letters and digits only.
fn plain(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// A phone number as Meta hashes it: its digits with the country code,
/// without the plus or leading zeros. Only one written with its country
/// code (`+33…`) can be told.
fn phone(number: &str) -> Option<String> {
    let digits: String = number
        .trim()
        .strip_prefix('+')?
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let digits = digits.trim_start_matches('0');
    (8..=15).contains(&digits.len()).then(|| digits.to_owned())
}

/// Meta's click id as its pixel would keep it: the one the browser had,
/// else rebuilt from the `fbclid` of the arrival the order remembers
/// (`fb.1.<milliseconds>.<fbclid>`), for a shopper who allowed the cookies
/// only after landing.
fn click_id(order: &OrderState, browser: &Browser) -> Option<String> {
    browser.fbc.clone().or_else(|| {
        let click = &order.source.as_ref()?.last;
        let fbclid = click.fbclid.as_deref()?;
        Some(format!("fb.1.{}.{fbclid}", click.at * 1000))
    })
}

/// The buyer as the Conversions API matches them: the e-mail address, the
/// phone, the name, the city, the postal code and the country hashed (hex
/// SHA-256 of the value normalized as Meta says), the browser's own ids as
/// they are.
fn user_data(order: &OrderState, browser: &Browser) -> Value {
    let mut data = Map::new();
    let mut hashed = |key: &str, value: String| {
        if !value.is_empty() {
            data.insert(key.into(), json!([sha256(&value)]));
        }
    };
    hashed("em", order.email.trim().to_lowercase());
    if let Some(phone) = order.shipping.phone.as_deref().and_then(phone) {
        hashed("ph", phone);
    }
    let name = order.shipping.name.trim();
    let (first, last) = name.split_once(' ').unwrap_or((name, ""));
    hashed("fn", plain(first));
    hashed("ln", plain(last));
    hashed("ct", plain(&order.shipping.city));
    hashed("zp", plain(&order.shipping.postal_code));
    hashed("country", order.shipping.country.trim().to_lowercase());
    if let Some(user_agent) = &browser.user_agent {
        data.insert("client_user_agent".into(), json!(user_agent));
    }
    if let Some(fbp) = &browser.fbp {
        data.insert("fbp".into(), json!(fbp));
    }
    if let Some(fbc) = click_id(order, browser) {
        data.insert("fbc".into(), json!(fbc));
    }
    Value::Object(data)
}

/// The sale of `order` as a Conversions API event: what the storefront's
/// pixel sends as `Purchase`, under the same event id (the order's), with
/// the buyer for Meta to match.
pub fn purchase_event(order: &OrderState, browser: &Browser) -> Value {
    let amount = |cents: u64| json!(cents as f64 / 100.0);
    let id = |line: &timada_orders::OrderLine| {
        line.sku.clone().unwrap_or_else(|| line.product_id.clone())
    };
    let mut event = Map::new();
    event.insert("event_name".into(), json!("Purchase"));
    event.insert("event_time".into(), json!(order.placed_at));
    event.insert("event_id".into(), json!(order.id));
    event.insert("action_source".into(), json!("website"));
    if let Some(page) = &browser.page {
        event.insert("event_source_url".into(), json!(page));
    }
    event.insert("user_data".into(), user_data(order, browser));
    event.insert(
        "custom_data".into(),
        json!({
            "currency": order.currency,
            "value": amount(order.total_cents),
            "order_id": order.reference,
            "content_type": "product",
            "content_ids": order.lines.iter().map(id).collect::<Vec<_>>(),
            "contents": order
                .lines
                .iter()
                .map(|line| json!({
                    "id": id(line),
                    "quantity": line.quantity,
                    "item_price": amount(line.unit_amount_cents),
                }))
                .collect::<Vec<_>>(),
            "num_items": order.lines.iter().map(|line| u64::from(line.quantity)).sum::<u64>(),
        }),
    );
    Value::Object(event)
}

#[evento::subscription]
async fn on_browser_set<E: Executor>(
    context: &Context<'_, E>,
    event: Event<BrowserSet>,
) -> anyhow::Result<()> {
    let conversions: Arc<Conversions> = context.extract();
    let store: TenantStore = context.extract();
    tell(
        &conversions,
        &store.0,
        &event.tenant,
        &event.aggregate_id,
        crate::auth::now(),
    )
    .await
}

/// Tells Meta the sale of `order_id`, if the shop has a pixel and a token
/// and the sale is young enough; see the module docs for what an answer
/// leads to.
async fn tell(
    conversions: &Conversions,
    store: &Evento,
    tenant: &str,
    order_id: &str,
    now: u64,
) -> anyhow::Result<()> {
    let pages = timada_pages::Command::new(store.clone(), tenant);
    let Some(pixel) = pages.load_meta_pixel().await? else {
        return Ok(());
    };
    let (Some(pixel_id), Some(sealed)) = (&pixel.pixel_id, &pixel.token_sealed) else {
        return Ok(());
    };
    let token = match conversions
        .sealer
        .as_ref()
        .map(|sealer| sealer.open_for(tenant, sealed))
    {
        Some(Ok(token)) => token,
        // The operator's secret changed, or was taken off: the merchant
        // sets the token again.
        Some(Err(_)) | None => {
            tracing::warn!(tenant, "a shop's Conversions API token cannot be opened");
            return Ok(());
        }
    };
    let orders = timada_orders::Command::new(store.clone(), tenant);
    let Some(order) = orders.load_order(order_id).await? else {
        return Ok(());
    };
    let Some(browser) = &order.browser else {
        return Ok(());
    };
    let age = now.saturating_sub(order.placed_at);
    if age > MAX_EVENT_AGE_SECS {
        return Ok(());
    }
    let event = purchase_event(&order, browser);
    match conversions.meta.send(pixel_id, &token, &event).await {
        MetaAnswer::Taken => Ok(()),
        MetaAnswer::Refused(reason) => {
            tracing::warn!(tenant, order_id, reason, "Meta refused a sale");
            pages
                .refuse_meta_conversion(ISSUER, order_id, &reason)
                .await?;
            Ok(())
        }
        MetaAnswer::Unavailable(reason) if age > GIVE_UP_AFTER_SECS => {
            tracing::warn!(
                tenant,
                order_id,
                reason,
                "Meta did not take a sale in time: left to the browser's event"
            );
            Ok(())
        }
        MetaAnswer::Unavailable(reason) => Err(anyhow::anyhow!(
            "Meta did not take the sale of order {order_id}: {reason}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use timada_orders::{Click, OrderLine, OrderSource, PROVIDER_STRIPE, Payment, ShippingAddress};
    use timada_pages::TokenChange;

    use super::*;

    const TENANT: &str = "acme";
    const ORDER: &str = "order-1";
    const PIXEL: &str = "1234567890";
    const TOKEN: &str = "EAAtoken";

    /// Meta as the test plays it: answers what it is told to, and keeps
    /// what it was sent.
    struct FakeMeta {
        answer: Mutex<MetaAnswer>,
        sent: Mutex<Vec<(String, String, Value)>>,
    }

    impl FakeMeta {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                answer: Mutex::new(MetaAnswer::Taken),
                sent: Mutex::new(Vec::new()),
            })
        }

        fn answers(&self, answer: MetaAnswer) {
            *self.answer.lock().unwrap() = answer;
        }

        fn sent(&self) -> Vec<(String, String, Value)> {
            self.sent.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Meta for FakeMeta {
        async fn send(&self, pixel_id: &str, token: &str, event: &Value) -> MetaAnswer {
            self.sent
                .lock()
                .unwrap()
                .push((pixel_id.to_owned(), token.to_owned(), event.clone()));
            self.answer.lock().unwrap().clone()
        }
    }

    fn sealer() -> Sealer {
        Sealer::new("meta", "secret")
    }

    fn browser() -> Browser {
        Browser {
            user_agent: Some("Mozilla/5.0".into()),
            fbp: Some("fb.1.1800000000000.42".into()),
            fbc: None,
            page: Some("https://shop.example/checkout".into()),
        }
    }

    async fn store() -> Evento {
        Evento::new(evento::Turso::in_memory_concurrent().await.unwrap())
    }

    /// An order placed just now, whose checkout gave `browser`.
    async fn place(store: &Evento, order_id: &str, browser: Option<Browser>) {
        let mut write = evento::append(order_id);
        write
            .tenant(TENANT)
            .original_version(0)
            .event(&timada_orders::aggregator::Placed {
                reference: "K7Q4-MZP2".to_owned(),
                cart_id: "cart-1".to_owned(),
                email: " Ada@Example.org ".to_owned(),
                shipping: ShippingAddress {
                    name: "Ada King Lovelace".into(),
                    line1: "1 rue de la Paix".into(),
                    postal_code: "75 002".into(),
                    city: "Saint-Étienne".into(),
                    country: "FR".into(),
                    phone: Some("+33 6 12 34 56 78".into()),
                    ..Default::default()
                },
                note: None,
                lines: vec![OrderLine {
                    product_id: "product-1".into(),
                    name: "Mug".into(),
                    sku: Some("MUG-BLUE".into()),
                    unit_amount_cents: 1250,
                    quantity: 2,
                    ..Default::default()
                }],
                currency: "EUR".to_owned(),
                total_cents: 2990,
                payment: Payment {
                    provider: PROVIDER_STRIPE.to_owned(),
                    payment_id: "pi_1".to_owned(),
                    amount_cents: 2990,
                    currency: "EUR".to_owned(),
                    capture_before: None,
                },
                supplier: None,
                placed_by: "storefront".to_owned(),
            })
            .event(&timada_orders::aggregator::SourceSet {
                source: OrderSource {
                    first: Click::default(),
                    last: Click {
                        fbclid: Some("IwAR0abc".into()),
                        at: 1_800_000_000,
                        ..Click::default()
                    },
                },
                set_by: "storefront".to_owned(),
            });
        if let Some(browser) = browser {
            write.event(&BrowserSet {
                browser,
                set_by: "storefront".to_owned(),
            });
        }
        write.commit(store).await.unwrap();
    }

    async fn set_pixel(store: &Evento, token: bool) {
        let change = if token {
            TokenChange::Set(sealer().seal_for(TENANT, TOKEN).unwrap())
        } else {
            TokenChange::Remove
        };
        timada_pages::Command::new(store.clone(), TENANT)
            .set_meta_pixel("alice", Some(PIXEL), change)
            .await
            .unwrap();
    }

    fn saga(meta: &Arc<FakeMeta>, store: &Evento) -> SubscriptionBuilder<Evento> {
        subscription(
            Arc::new(Conversions::new(Some(sealer()), meta.clone())),
            store.clone(),
        )
    }

    #[tokio::test]
    async fn a_sale_is_told_under_the_orders_id_with_the_buyer_hashed() {
        let store = store().await;
        set_pixel(&store, true).await;
        place(&store, ORDER, Some(browser())).await;
        let meta = FakeMeta::new();
        saga(&meta, &store).run_once(&store).await.unwrap();

        let sent = meta.sent();
        assert_eq!(sent.len(), 1);
        let (pixel_id, token, event) = &sent[0];
        assert_eq!(pixel_id, PIXEL);
        assert_eq!(token, TOKEN);
        assert_eq!(event["event_name"], "Purchase");
        assert_eq!(event["event_id"], ORDER);
        assert_eq!(event["action_source"], "website");
        assert_eq!(event["event_source_url"], "https://shop.example/checkout");
        let user = &event["user_data"];
        assert_eq!(user["em"], json!([sha256("ada@example.org")]));
        assert_eq!(user["ph"], json!([sha256("33612345678")]));
        assert_eq!(user["fn"], json!([sha256("ada")]));
        assert_eq!(user["ln"], json!([sha256("kinglovelace")]));
        assert_eq!(user["ct"], json!([sha256("saintétienne")]));
        assert_eq!(user["zp"], json!([sha256("75002")]));
        assert_eq!(user["country"], json!([sha256("fr")]));
        assert_eq!(user["client_user_agent"], "Mozilla/5.0");
        assert_eq!(user["fbp"], "fb.1.1800000000000.42");
        // No `_fbc` cookie: rebuilt from the click the order remembers.
        assert_eq!(user["fbc"], "fb.1.1800000000000.IwAR0abc");
        let custom = &event["custom_data"];
        assert_eq!(custom["value"], 29.9);
        assert_eq!(custom["currency"], "EUR");
        assert_eq!(custom["order_id"], "K7Q4-MZP2");
        assert_eq!(custom["content_ids"], json!(["MUG-BLUE"]));
        assert_eq!(
            custom["contents"],
            json!([{ "id": "MUG-BLUE", "quantity": 2, "item_price": 12.5 }])
        );
        assert_eq!(custom["num_items"], 2);
    }

    #[tokio::test]
    async fn nothing_is_told_without_the_shoppers_leave_or_the_shops_token() {
        // The shopper did not allow advertising: the order has no browser.
        let store = store().await;
        set_pixel(&store, true).await;
        place(&store, ORDER, None).await;
        let meta = FakeMeta::new();
        saga(&meta, &store).run_once(&store).await.unwrap();
        assert!(meta.sent().is_empty());

        // A pixel without a token: the browser's event alone.
        let store = self::store().await;
        set_pixel(&store, false).await;
        place(&store, ORDER, Some(browser())).await;
        saga(&meta, &store).run_once(&store).await.unwrap();
        assert!(meta.sent().is_empty());

        // No pixel at all.
        let store = self::store().await;
        place(&store, ORDER, Some(browser())).await;
        saga(&meta, &store).run_once(&store).await.unwrap();
        assert!(meta.sent().is_empty());

        // A token sealed under another secret is not sent as it is.
        let store = self::store().await;
        set_pixel(&store, true).await;
        place(&store, ORDER, Some(browser())).await;
        subscription::<Evento>(
            Arc::new(Conversions::new(
                Some(Sealer::new("meta", "another secret")),
                meta.clone(),
            )),
            store.clone(),
        )
        .run_once(&store)
        .await
        .unwrap();
        assert!(meta.sent().is_empty());
    }

    #[tokio::test]
    async fn a_refusal_is_kept_for_the_merchant_and_the_pass_goes_on() {
        let store = store().await;
        set_pixel(&store, true).await;
        place(&store, ORDER, Some(browser())).await;
        place(&store, "order-2", Some(browser())).await;
        let meta = FakeMeta::new();
        meta.answers(MetaAnswer::Refused(
            "Invalid OAuth access token (code 190)".into(),
        ));
        saga(&meta, &store).run_once(&store).await.unwrap();
        // Both were tried: the first refusal held nothing up.
        assert_eq!(meta.sent().len(), 2);
        let pixel = timada_pages::Command::new(store.clone(), TENANT)
            .load_meta_pixel()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            pixel.refusal.as_deref(),
            Some("Invalid OAuth access token (code 190)")
        );
    }

    #[tokio::test]
    async fn an_answer_that_does_not_come_is_asked_for_again_for_an_hour() {
        let store = store().await;
        set_pixel(&store, true).await;
        place(&store, ORDER, Some(browser())).await;
        let meta = FakeMeta::new();
        meta.answers(MetaAnswer::Unavailable("timed out".into()));
        let conversions = Conversions::new(Some(sealer()), meta.clone());
        let now = crate::auth::now();

        // Young: the pass fails, to be tried again.
        let failed = tell(&conversions, &store, TENANT, ORDER, now).await;
        assert!(failed.is_err());
        // Past the hour: left to the browser's event.
        tell(
            &conversions,
            &store,
            TENANT,
            ORDER,
            now + GIVE_UP_AFTER_SECS + 60,
        )
        .await
        .unwrap();
        assert_eq!(meta.sent().len(), 2);
        // Too old for Meta to take: not sent at all.
        tell(
            &conversions,
            &store,
            TENANT,
            ORDER,
            now + MAX_EVENT_AGE_SECS + 60,
        )
        .await
        .unwrap();
        assert_eq!(meta.sent().len(), 2);
        // Nothing was written about it.
        let pixel = timada_pages::Command::new(store.clone(), TENANT)
            .load_meta_pixel()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(pixel.refusal, None);
    }

    #[test]
    fn a_refusal_reads_metas_words() {
        let body = br#"{"error":{"message":"Invalid OAuth access token.","type":"OAuthException","code":190}}"#;
        assert_eq!(refusal(body, 400), "Invalid OAuth access token. (code 190)");
        assert_eq!(refusal(b"<html>", 403), "HTTP 403");
    }

    #[test]
    fn a_phone_is_told_only_with_its_country_code() {
        assert_eq!(phone("+33 6 12 34 56 78").as_deref(), Some("33612345678"));
        assert_eq!(phone("06 12 34 56 78"), None);
        assert_eq!(phone("+33"), None);
    }
}
