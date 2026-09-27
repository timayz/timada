//! AliExpress as a [`SupplierConnector`] (feature `aliexpress`): what an item
//! costs and how many it holds, dropshipping orders placed against it, and
//! the logistics tracking that comes back. A thin client over their TOP
//! gateway — no SDK.
//!
//! **This has never been run against the real API.** The shape follows their
//! published parameters, and the tests answer it from a local server; the
//! first round trip against a real app key is the host's, and will very
//! likely need the field names adjusted. Keep that in mind before trusting
//! it with a catalogue.
//!
//! Two things are worth knowing about their platform. Signing is
//! HMAC-SHA256 over the sorted parameters with no separators — get the sort
//! wrong and every call comes back as a signature error, which is why
//! [`sign`] is public and tested on its own. And a business failure arrives
//! with **HTTP 200 and an error code in the body**, so a status check alone
//! would take "this item no longer exists" for success.

use std::{collections::BTreeMap, time::Duration};

use hmac::{Hmac, Mac};
use serde::Deserialize;
use sha2::Sha256;
use timada_core::Money;

use crate::connector::{
    ConnectorError, ConnectorFuture, ConnectorLimits, ConnectorTask, PlaceOrder, PlacedOrder,
    SupplierConnector, SupplierItemRef, SupplierOffer, SupplierOrderStanding,
};

/// Their gateway.
pub const API_BASE: &str = "https://api-sg.aliexpress.com/sync";

/// The keys of an AliExpress app. `Debug` never shows the secrets.
#[derive(Clone)]
pub struct AliExpressConfig {
    pub app_key: String,
    pub app_secret: String,
    /// The token the shop's own account was authorised with.
    pub access_token: String,
    /// What currency their answers quote in for this account.
    pub currency: String,
    /// The gateway, unless a test points elsewhere.
    pub api_base: String,
    /// How many items one `offers` call asks about, and the gap between two
    /// calls. Their published limits differ per app, so the host sets them.
    pub limits: ConnectorLimits,
}

impl AliExpressConfig {
    pub fn new(
        app_key: impl Into<String>,
        app_secret: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            app_key: app_key.into(),
            app_secret: app_secret.into(),
            access_token: access_token.into(),
            currency: "USD".to_owned(),
            api_base: API_BASE.to_owned(),
            limits: ConnectorLimits {
                batch: 20,
                min_interval: Duration::from_secs(1),
            },
        }
    }

    pub fn with_api_base(mut self, api_base: impl Into<String>) -> Self {
        self.api_base = api_base.into();
        self
    }

    pub fn with_currency(mut self, currency: impl Into<String>) -> Self {
        self.currency = currency.into();
        self
    }

    pub fn with_limits(mut self, limits: ConnectorLimits) -> Self {
        self.limits = limits;
        self
    }
}

impl std::fmt::Debug for AliExpressConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AliExpressConfig")
            .field("app_key", &self.app_key)
            .field("api_base", &self.api_base)
            .field("currency", &self.currency)
            .finish_non_exhaustive()
    }
}

/// Signs a call the way the TOP gateway wants it: the parameters sorted by
/// name, concatenated as `namevalue` with nothing between them, HMAC-SHA256
/// under the app secret, upper-case hex.
///
/// Public because it is the one part that cannot be debugged from the outside:
/// a wrong signature is indistinguishable from a wrong key.
pub fn sign(app_secret: &str, params: &BTreeMap<String, String>) -> String {
    let mut payload = String::new();
    for (name, value) in params {
        payload.push_str(name);
        payload.push_str(value);
    }
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()) else {
        return String::new();
    };
    mac.update(payload.as_bytes());
    hex::encode_upper(mac.finalize().into_bytes())
}

#[derive(Debug, Clone)]
pub struct AliExpressConnector {
    http: reqwest::Client,
    config: AliExpressConfig,
}

/// What every answer carries when something went wrong — with HTTP 200.
#[derive(Debug, Deserialize)]
struct Fault {
    code: Option<String>,
    #[serde(alias = "msg")]
    message: Option<String>,
    /// Their per-method result envelopes repeat the code under other names.
    #[serde(alias = "sub_code")]
    subcode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(flatten)]
    fault: Fault,
    #[serde(flatten)]
    result: Option<T>,
}

#[derive(Debug, Deserialize)]
struct ItemsResult {
    #[serde(default)]
    items: Vec<Item>,
}

#[derive(Debug, Deserialize)]
struct Item {
    product_id: String,
    #[serde(default)]
    sku_id: Option<String>,
    /// Their money fields come as decimal strings.
    #[serde(default)]
    sku_price: Option<String>,
    #[serde(default)]
    shipping_price: Option<String>,
    #[serde(default)]
    sku_stock: Option<i64>,
    #[serde(default)]
    product_title: Option<String>,
    #[serde(default)]
    product_detail_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PlacedResult {
    order_id: String,
    #[serde(default)]
    total_amount: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StandingResult {
    /// `PLACE_ORDER_SUCCESS`, `WAIT_SELLER_SEND_GOODS`, `SELLER_SEND_GOODS`,
    /// `FINISH`, `ORDER_CANCEL`, …
    order_status: String,
    #[serde(default)]
    logistics_service_name: Option<String>,
    #[serde(default)]
    logistics_no: Option<String>,
}

impl AliExpressConnector {
    pub fn new(config: AliExpressConfig) -> Result<Self, ConnectorError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| ConnectorError::Unavailable(err.to_string()))?;
        Ok(Self { http, config })
    }

    /// One signed call, with their business faults read out of the body.
    async fn call<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        mut params: BTreeMap<String, String>,
    ) -> Result<T, ConnectorError> {
        params.insert("method".into(), method.to_owned());
        params.insert("app_key".into(), self.config.app_key.clone());
        params.insert("session".into(), self.config.access_token.clone());
        params.insert("sign_method".into(), "hmac-sha256".into());
        params.insert("format".into(), "json".into());
        params.insert("v".into(), "2.0".into());
        params.insert("timestamp".into(), timestamp()?);
        let signature = sign(&self.config.app_secret, &params);
        params.insert("sign".into(), signature);

        let response = self
            .http
            .post(self.config.api_base.trim_end_matches('/'))
            .form(&params)
            .send()
            .await
            .map_err(|err| ConnectorError::Unavailable(err.to_string()))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|err| ConnectorError::Unavailable(err.to_string()))?;

        if !status.is_success() {
            return Err(http_error(status, &body));
        }
        // A 200 proves nothing here: the fault is in the body.
        let envelope: Envelope<T> = serde_json::from_slice(&body)
            .map_err(|err| ConnectorError::Unavailable(format!("unreadable answer: {err}")))?;
        if let Some(err) = envelope.fault.into_error() {
            return Err(err);
        }
        envelope.result.ok_or_else(|| {
            ConnectorError::Unavailable("answer carried neither a result nor a fault".to_owned())
        })
    }

    fn money(&self, raw: Option<&str>) -> Money {
        Money::new(minor_units(raw.unwrap_or("0")), &self.config.currency)
    }
}

impl Fault {
    /// Their codes, mapped onto what a worker can do about them.
    fn into_error(self) -> Option<ConnectorError> {
        let code = self.subcode.or(self.code)?;
        if code.is_empty() || code == "0" {
            return None;
        }
        let message = self
            .message
            .unwrap_or_else(|| format!("AliExpress said {code}"));
        let upper = code.to_ascii_uppercase();
        // Throttled: their app-level and method-level limiters both land here.
        if upper.contains("TRAFFIC_LIMIT") || upper.contains("APP_CALL_LIMITED") {
            return Some(ConnectorError::RateLimited { retry_after: 60 });
        }
        // Their platform having a bad day, or a token to refresh: try later.
        if upper.starts_with("ISP.") || upper.contains("SYSTEM_ERROR") || upper.contains("TOKEN") {
            return Some(ConnectorError::Unavailable(message));
        }
        if upper.contains("PRODUCT_NOT_EXIST") || upper.contains("ITEM_NOT_FOUND") {
            return Some(ConnectorError::UnknownItem(message));
        }
        Some(ConnectorError::Refused(message))
    }
}

/// An HTTP failure, before their body could say anything.
fn http_error(status: reqwest::StatusCode, body: &[u8]) -> ConnectorError {
    let detail: String = String::from_utf8_lossy(body).chars().take(200).collect();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return ConnectorError::RateLimited { retry_after: 60 };
    }
    if status.is_server_error() || status == reqwest::StatusCode::UNAUTHORIZED {
        return ConnectorError::Unavailable(format!("HTTP {status}: {detail}"));
    }
    ConnectorError::Refused(format!("HTTP {status}: {detail}"))
}

/// `"12.34"` → `1234`. Their decimal strings, in minor units, rounded to the
/// nearest cent — a price is not the place to truncate.
fn minor_units(raw: &str) -> i64 {
    let raw = raw.trim();
    let negative = raw.starts_with('-');
    let digits = raw.trim_start_matches(['-', '+']);
    let (units, fraction) = digits.split_once('.').unwrap_or((digits, ""));
    let units: i64 = units.parse().unwrap_or(0);
    let mut cents: i64 = fraction
        .chars()
        .chain(std::iter::repeat('0'))
        .take(2)
        .filter(char::is_ascii_digit)
        .fold(0, |acc, digit| {
            acc * 10 + i64::from(digit.to_digit(10).unwrap_or(0))
        });
    // A third decimal rounds the second.
    if fraction
        .chars()
        .nth(2)
        .and_then(|digit| digit.to_digit(10))
        .is_some_and(|digit| digit >= 5)
    {
        cents += 1;
    }
    let minor = units.saturating_mul(100).saturating_add(cents);
    if negative { -minor } else { minor }
}

fn timestamp() -> Result<String, ConnectorError> {
    timada_core::time::now_unix_secs()
        .map(|secs| (secs * 1_000).to_string())
        .map_err(|err| ConnectorError::Unavailable(err.to_string()))
}

impl SupplierConnector for AliExpressConnector {
    fn key(&self) -> &str {
        "aliexpress"
    }

    fn does(&self, _task: ConnectorTask) -> bool {
        // All three: quoting, ordering and tracking. Calling an order *off*
        // is not in their dropshipping API — that is done on the platform —
        // but cancelling is not a task of its own, and `cancel` says so
        // plainly rather than the whole connector claiming it cannot track.
        true
    }

    fn limits(&self) -> ConnectorLimits {
        self.config.limits
    }

    fn offers<'a>(
        &'a self,
        items: &'a [SupplierItemRef],
    ) -> ConnectorFuture<'a, Vec<SupplierOffer>> {
        Box::pin(async move {
            if items.is_empty() {
                return Ok(Vec::new());
            }
            let ids: Vec<&str> = items
                .iter()
                .map(|item| item.external_item_id.as_str())
                .collect();
            let mut params = BTreeMap::new();
            params.insert("product_ids".into(), ids.join(","));
            params.insert("target_currency".into(), self.config.currency.clone());
            let result: ItemsResult = self.call("aliexpress.ds.product.get", params).await?;

            // Their answer is per (item, sku); an item the shop asked about
            // and does not get back is simply left out, which the sync
            // worker already reads as "no longer listed".
            let mut offers = Vec::new();
            for asked in items {
                let Some(item) = result.items.iter().find(|item| {
                    item.product_id == asked.external_item_id
                        && match (&asked.external_sku, &item.sku_id) {
                            (Some(wanted), Some(got)) => wanted == got,
                            (Some(_), None) => false,
                            (None, _) => true,
                        }
                }) else {
                    continue;
                };
                offers.push(SupplierOffer {
                    item: asked.clone(),
                    cost: self.money(item.sku_price.as_deref()),
                    shipping: self.money(item.shipping_price.as_deref()),
                    available: item.sku_stock.unwrap_or(0).clamp(0, i64::from(u32::MAX)) as u32,
                    title: item.product_title.clone(),
                    url: item.product_detail_url.clone(),
                });
            }
            Ok(offers)
        })
    }

    fn place<'a>(&'a self, order: &'a PlaceOrder<'a>) -> ConnectorFuture<'a, PlacedOrder> {
        Box::pin(async move {
            let address = &order.ship_to;
            let products: Vec<serde_json::Value> = order
                .lines
                .iter()
                .map(|line| {
                    serde_json::json!({
                        "product_id": line.item.external_item_id,
                        "sku_attr": line.item.external_sku,
                        "product_count": line.quantity,
                    })
                })
                .collect();
            let request = serde_json::json!({
                // Their idempotency: the shop's own purchase id, so a retry
                // after a lost answer is the same order, not a second one.
                "out_order_id": order.reference,
                "logistics_address": {
                    "contact_person": format!("{} {}", address.first_name, address.last_name),
                    "address": address.line1,
                    "address2": address.line2,
                    "city": address.city,
                    "zip": address.postal_code,
                    "country": address.country_code,
                    "phone_country": "",
                    "mobile_no": address.mobile.clone().or_else(|| address.phone.clone()),
                },
                "product_items": products,
                "remark": order.note,
            });
            let mut params = BTreeMap::new();
            params.insert(
                "param_place_order_request4_open_api_d_t_o".into(),
                request.to_string(),
            );
            let placed: PlacedResult = self.call("aliexpress.ds.order.create", params).await?;
            Ok(PlacedOrder {
                external_order_id: placed.order_id,
                cost: self.money(placed.total_amount.as_deref()),
            })
        })
    }

    fn standing<'a>(
        &'a self,
        external_order_id: &'a str,
    ) -> ConnectorFuture<'a, SupplierOrderStanding> {
        Box::pin(async move {
            let mut params = BTreeMap::new();
            params.insert("order_id".into(), external_order_id.to_owned());
            let standing: StandingResult =
                self.call("aliexpress.ds.trade.order.get", params).await?;
            let status = standing.order_status.to_ascii_uppercase();
            if status.contains("CANCEL") || status.contains("CLOSED") {
                return Ok(SupplierOrderStanding::Cancelled {
                    reason: standing.order_status,
                });
            }
            // Shipped is a tracking number, not a status: a parcel with no
            // number to give the customer is still on its way as far as the
            // shop is concerned.
            match (standing.logistics_no, standing.logistics_service_name) {
                (Some(tracking), carrier) if !tracking.trim().is_empty() => {
                    Ok(SupplierOrderStanding::Shipped {
                        carrier: carrier.unwrap_or_else(|| "AliExpress".to_owned()),
                        tracking_number: tracking,
                    })
                }
                _ => Ok(SupplierOrderStanding::Pending),
            }
        })
    }

    fn cancel<'a>(&'a self, _external_order_id: &'a str) -> ConnectorFuture<'a, ()> {
        Box::pin(async {
            Err(ConnectorError::Refused(
                "an AliExpress order is called off on the platform, not through the API".to_owned(),
            ))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_the_sorted_parameters() {
        let mut params = BTreeMap::new();
        params.insert("method".to_owned(), "aliexpress.ds.product.get".to_owned());
        params.insert("app_key".to_owned(), "12345".to_owned());
        params.insert("timestamp".to_owned(), "1789776000000".to_owned());
        // Sorted, so app_key comes first whatever order they went in.
        let signed = sign("a-secret", &params);
        assert_eq!(signed.len(), 64);
        assert_eq!(signed, signed.to_uppercase());
        // The same parameters in another order sign the same.
        let mut shuffled = BTreeMap::new();
        shuffled.insert("timestamp".to_owned(), "1789776000000".to_owned());
        shuffled.insert("app_key".to_owned(), "12345".to_owned());
        shuffled.insert("method".to_owned(), "aliexpress.ds.product.get".to_owned());
        assert_eq!(sign("a-secret", &shuffled), signed);
        // A different secret does not.
        assert_ne!(sign("another-secret", &params), signed);
    }

    #[test]
    fn reads_their_decimal_strings() {
        assert_eq!(minor_units("12.34"), 1_234);
        assert_eq!(minor_units("12.3"), 1_230);
        assert_eq!(minor_units("12"), 1_200);
        assert_eq!(minor_units("0.07"), 7);
        assert_eq!(minor_units(""), 0);
        // Rounded, not truncated.
        assert_eq!(minor_units("1.005"), 101);
        assert_eq!(minor_units("1.004"), 100);
        assert_eq!(minor_units("-3.50"), -350);
    }

    #[test]
    fn their_codes_say_what_to_do_about_them() {
        let fault = |code: &str| Fault {
            code: Some(code.to_owned()),
            message: Some("something".to_owned()),
            subcode: None,
        };
        assert!(fault("0").into_error().is_none());
        assert!(matches!(
            fault("APP_CALL_LIMITED").into_error(),
            Some(ConnectorError::RateLimited { .. })
        ));
        assert!(matches!(
            fault("ISP.TOP-REMOTE-SERVICE-UNAVAILABLE").into_error(),
            Some(ConnectorError::Unavailable(_))
        ));
        assert!(matches!(
            fault("INVALID_TOKEN").into_error(),
            Some(ConnectorError::Unavailable(_))
        ));
        assert!(matches!(
            fault("PRODUCT_NOT_EXIST").into_error(),
            Some(ConnectorError::UnknownItem(_))
        ));
        assert!(matches!(
            fault("ADDRESS_NOT_SUPPORTED").into_error(),
            Some(ConnectorError::Refused(_))
        ));
    }
}
