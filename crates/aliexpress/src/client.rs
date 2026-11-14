//! The calls to AliExpress's open platform, signed the way it wants them:
//! every parameter, sorted by name and run together as `name` + `value`
//! (after the API path, for the calls addressed by path), HMAC-SHA256 under
//! the app's secret, in upper-case hex. Dotted methods
//! (`aliexpress.ds.product.get`) go to `<api>/sync` and name themselves in
//! `method`; path methods (`/auth/token/create`) go to `<api>/rest<path>`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use timada_orders::Checkpoint;

use crate::address::AddressTree;
use crate::forwarding::{choose_service, describe_options};
use crate::model::{AeOrder, AeProduct, Delivery, FreightOption, Market, Tokens};

/// AliExpress's own gateway.
pub const DEFAULT_API_URL: &str = "https://api-sg.aliexpress.com";
/// Where an AliExpress account says yes to an app.
pub const DEFAULT_AUTHORIZE_URL: &str = "https://api-sg.aliexpress.com/oauth/authorize";

/// Why a call came back without what it asked for.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("AliExpress refused ({code}): {message}")]
pub struct AeError {
    /// AliExpress's own code, or `http_<status>` / `network` / `response`.
    pub code: String,
    pub message: String,
    /// Worth trying again later (the network, a limit, AliExpress itself).
    pub retryable: bool,
    /// The item is no longer sold (removed, or never was).
    pub gone: bool,
}

impl AeError {
    pub(crate) fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        let code = code.into();
        let message = message.into();
        let lowered = format!("{code} {message}").to_ascii_lowercase();
        let retryable = lowered.contains("limit")
            || lowered.contains("frequency")
            || lowered.contains("timeout")
            || lowered.contains("isp.")
            || lowered.contains("system error");
        let gone = lowered.contains("not exist")
            || lowered.contains("not found")
            || lowered.contains("offline")
            || lowered.contains("off shelf")
            || lowered.contains("off-shelf");
        Self {
            code,
            message,
            retryable,
            gone,
        }
    }

    /// Whether placing an order again could buy the goods twice: anything
    /// but a plain refusal leaves that open.
    pub fn may_have_placed(&self) -> bool {
        self.retryable || self.code == "response"
    }

    fn transient(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            retryable: true,
            gone: false,
        }
    }
}

/// An app's client: its key and secret, and the token of the account it
/// acts for once one said yes.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    api_url: String,
    app_key: String,
    app_secret: String,
    session: Option<String>,
    /// Counts every call made, when the host keeps count.
    calls: Option<Arc<AtomicU64>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("api_url", &self.api_url)
            .field("app_key", &self.app_key)
            .finish_non_exhaustive()
    }
}

/// The signature of a call: `path` for the calls addressed by path,
/// `params` every parameter but `sign`.
pub fn sign(app_secret: &str, path: Option<&str>, params: &BTreeMap<String, String>) -> String {
    let mut base = String::from(path.unwrap_or_default());
    for (name, value) in params {
        base.push_str(name);
        base.push_str(value);
    }
    let mut mac =
        Hmac::<Sha256>::new_from_slice(app_secret.as_bytes()).expect("HMAC takes any key length");
    mac.update(base.as_bytes());
    mac.finalize()
        .into_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect()
}

/// Where to send the merchant so their AliExpress account says yes to the
/// app; AliExpress sends them back to `redirect_uri` with `code` and
/// `state`.
pub fn authorize_url(base: &str, app_key: &str, redirect_uri: &str, state: &str) -> String {
    let mut url = reqwest::Url::parse(base).unwrap_or_else(|_| {
        reqwest::Url::parse(DEFAULT_AUTHORIZE_URL).expect("the default address parses")
    });
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("force_auth", "true")
        .append_pair("client_id", app_key)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state);
    url.to_string()
}

impl AeProduct {
    /// The product in the answer to `aliexpress.ds.product.get`, or why
    /// there is none.
    pub fn from_response(product_id: &str, body: &Value) -> Result<Self, AeError> {
        let response = body
            .get("aliexpress_ds_product_get_response")
            .unwrap_or(body);
        let code = response
            .get("rsp_code")
            .map(crate::model::text)
            .unwrap_or_default();
        if !code.is_empty() && code != "200" {
            let message = response
                .get("rsp_msg")
                .map(crate::model::text)
                .unwrap_or_default();
            return Err(AeError::new(code, message));
        }
        let result = response
            .get("result")
            .ok_or_else(|| AeError::new("response", "no product in the answer"))?;
        AeProduct::from_json(product_id, result)
            .ok_or_else(|| AeError::new("response", "the product could not be read"))
    }
}

/// Where an order goes, as AliExpress names its parts.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct LogisticsAddress {
    pub full_name: String,
    pub contact_person: String,
    pub address: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub address2: String,
    pub city: String,
    /// The region, as AliExpress lists it; it refuses an order without one.
    pub province: String,
    pub zip: String,
    /// ISO 3166 alpha-2.
    pub country: String,
    /// `+33`; empty when the shopper gave no phone.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub phone_country: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mobile_no: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub locale: String,
}

/// One combination an order buys.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct PlaceItem {
    pub product_id: String,
    pub sku_attr: String,
    pub product_count: u32,
    pub logistics_service_name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub order_memo: String,
}

/// An order to place on AliExpress for the account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceOrder {
    pub address: LogisticsAddress,
    pub items: Vec<PlaceItem>,
    /// Ask AliExpress to charge the account's saved payment method.
    pub try_to_pay: bool,
    /// ISO 4217; the account pays in it.
    pub pay_currency: String,
}

impl PlaceOrder {
    /// The two parameters `aliexpress.ds.order.create` takes, each a JSON
    /// text.
    pub fn params(&self) -> (String, String) {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|item| {
                let mut value = serde_json::to_value(item).unwrap_or_default();
                // AliExpress wants the product number as a number.
                if let (Some(object), Ok(id)) =
                    (value.as_object_mut(), item.product_id.parse::<u64>())
                {
                    object.insert("product_id".into(), Value::from(id));
                }
                value
            })
            .collect();
        let order = serde_json::json!({
            "logistics_address": self.address,
            "product_items": items,
        });
        let extend = serde_json::json!({
            "payment": {
                "pay_currency": self.pay_currency,
                "try_to_pay": if self.try_to_pay { "true" } else { "false" },
            },
            "trade_extra_param": { "business_model": "retail" },
        });
        (order.to_string(), extend.to_string())
    }
}

/// The AliExpress order numbers in the answer to `aliexpress.ds.order.create`
/// (one per seller), or why there are none.
pub fn placed_orders_from_response(body: &Value) -> Result<Vec<String>, AeError> {
    let response = body
        .get("aliexpress_trade_buy_placeorder_response")
        .or_else(|| body.get("aliexpress_ds_order_create_response"))
        .unwrap_or(body);
    let result = response
        .get("result")
        .ok_or_else(|| AeError::new("response", "no result in the answer"))?;
    let success = result
        .get("is_success")
        .map(crate::model::text)
        .is_some_and(|flag| flag.eq_ignore_ascii_case("true"));
    if !success {
        let code = result
            .get("error_code")
            .map(crate::model::text)
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| "refused".to_owned());
        let message = result
            .get("error_msg")
            .map(crate::model::text)
            .unwrap_or_default();
        let mut err = AeError::new(code, message);
        // A refusal placed nothing; asking again is the merchant's call.
        err.retryable = false;
        err.gone = false;
        return Err(err);
    }
    let orders: Vec<String> = crate::model::list(result.get("order_list"))
        .iter()
        .map(crate::model::text)
        .filter(|id| !id.is_empty())
        .collect();
    if orders.is_empty() {
        return Err(AeError::new("response", "no order number in the answer"));
    }
    Ok(orders)
}

/// How long a country's places may take: France's weigh over 2 MB, and the
/// client's usual 20 seconds count the whole answer.
const ADDRESS_TREE_TIMEOUT: Duration = Duration::from_secs(90);

/// An error and what caused it: reqwest's own says only "error decoding
/// response body", whether the body was cut off, timed out or malformed.
fn with_causes(err: &dyn std::error::Error) -> String {
    let mut text = err.to_string();
    let mut cause = err.source();
    while let Some(err) = cause {
        text.push_str(": ");
        text.push_str(&err.to_string());
        cause = err.source();
    }
    text
}

/// AliExpress's way of saying the delivery lookup does not know a province.
const INVALID_PROVINCE: &str = "INVALID_PROVINCE_NAME";

/// AliExpress's way of saying it delivers nowhere near the address, which is
/// an answer rather than a failure.
const NO_DELIVERY: &str = "DELIVERY_NOT_AVAILABLE_TO_YOUR_ADDRESS";

/// The ways AliExpress delivers in the answer to
/// `aliexpress.ds.freight.query`, or why there are none.
pub fn freight_from_response(body: &Value) -> Result<Vec<FreightOption>, AeError> {
    let response = body
        .get("aliexpress_ds_freight_query_response")
        .unwrap_or(body);
    let result = response.get("result").unwrap_or(response);
    let success = result
        .get("success")
        .map(crate::model::text)
        .is_none_or(|flag| flag.eq_ignore_ascii_case("true"));
    if !success {
        let read = |name| result.get(name).map(crate::model::text).unwrap_or_default();
        let (code, message) = (read("code"), read("msg"));
        if code.contains(NO_DELIVERY) || message.contains(NO_DELIVERY) {
            return Ok(Vec::new());
        }
        return Err(AeError::new("freight", message));
    }
    Ok(FreightOption::list_from_json(result))
}

/// What `aliexpress.logistics.ds.trackinginfo.query` is asked about: one
/// parcel of one AliExpress order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParcelQuery {
    /// The parcel number (`logistics_no`).
    pub number: String,
    /// The AliExpress order it belongs to (`out_ref`).
    pub ae_order_id: String,
    /// The delivery service the order was placed with (`service_name`,
    /// `CAINIAO_STANDARD`).
    pub service_name: String,
    /// Where it goes: ISO 3166-1 alpha-2 (`to_area`).
    pub country: String,
}

/// Whether a parcel's journey, as its carrier tells it (oldest step first),
/// ended at the door: its last step says the parcel was delivered or signed
/// for, and not that it was not. AliExpress words the steps in English
/// (`Delivered`, `Signed by recipient`, `Delivered to pickup point`) and
/// gives no status beside them; a wording this misses only has the parcel
/// followed on until its time is up ([`crate::parcels_due`]). The wording
/// awaits the real-app test (#253).
pub fn parcel_delivered(checkpoints: &[Checkpoint]) -> bool {
    let Some(last) = checkpoints.last() else {
        return false;
    };
    let said = last.description.to_ascii_lowercase();
    let ended = said.contains("delivered") || said.contains("signed");
    let undone = [
        "not delivered",
        "undelivered",
        "unsuccessful",
        "fail",
        "returned",
    ]
    .iter()
    .any(|word| said.contains(word));
    ended && !undone
}

/// The steps of a parcel's journey in the answer to
/// `aliexpress.logistics.ds.trackinginfo.query` (`details.details[]`, each
/// an `event_date`, an `address` and an `event_desc`), oldest first as
/// AliExpress lists them, or why there are none. The shapes follow ae_sdk,
/// read leniently; a step with neither a date nor a description is dropped.
pub fn parcel_events_from_response(body: &Value) -> Result<Vec<Checkpoint>, AeError> {
    let response = body
        .get("aliexpress_logistics_ds_trackinginfo_query_response")
        .unwrap_or(body);
    if response.get("result_success") == Some(&Value::Bool(false)) {
        let code = response
            .get("result_error")
            .map(crate::model::text)
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| "tracking".to_owned());
        let message = response
            .get("error_desc")
            .map(crate::model::text)
            .unwrap_or_default();
        return Err(AeError::new(code, message));
    }
    let steps = crate::model::list(response.get("details"))
        .iter()
        .filter_map(|step| {
            let field = |name: &str| step.get(name).map(crate::model::text).unwrap_or_default();
            let checkpoint = Checkpoint {
                at: field("event_date"),
                place: field("address"),
                description: field("event_desc"),
            };
            (!checkpoint.at.is_empty() || !checkpoint.description.is_empty()).then_some(checkpoint)
        })
        .collect();
    Ok(steps)
}

impl AeOrder {
    /// The order in the answer to `aliexpress.trade.ds.order.get`, or why
    /// there is none.
    pub fn from_response(body: &Value) -> Result<Self, AeError> {
        let response = body
            .get("aliexpress_trade_ds_order_get_response")
            .or_else(|| body.get("aliexpress_ds_trade_order_get_response"))
            .unwrap_or(body);
        let code = response
            .get("rsp_code")
            .map(crate::model::text)
            .unwrap_or_default();
        if !code.is_empty() && code != "200" {
            let message = response
                .get("rsp_msg")
                .map(crate::model::text)
                .unwrap_or_default();
            return Err(AeError::new(code, message));
        }
        let result = response
            .get("result")
            .ok_or_else(|| AeError::new("response", "no order in the answer"))?;
        Ok(AeOrder::from_json(result))
    }
}

impl Client {
    pub fn new(
        http: reqwest::Client,
        api_url: &str,
        app_key: impl Into<String>,
        app_secret: impl Into<String>,
    ) -> Self {
        Self {
            http,
            api_url: api_url.trim_end_matches('/').to_owned(),
            app_key: app_key.into(),
            app_secret: app_secret.into(),
            session: None,
            calls: None,
        }
    }

    /// The same client counting each call it makes on `calls`: AliExpress
    /// counts them against the app, whichever account they act for.
    pub fn counting(mut self, calls: Arc<AtomicU64>) -> Self {
        self.calls = Some(calls);
        self
    }

    /// The same client acting for the account `access_token` stands for.
    pub fn with_session(mut self, access_token: impl Into<String>) -> Self {
        self.session = Some(access_token.into());
        self
    }

    pub fn app_key(&self) -> &str {
        &self.app_key
    }

    /// Exchanges the code AliExpress sent the merchant back with for the
    /// account's tokens.
    pub async fn create_token(&self, code: &str) -> Result<Tokens, AeError> {
        let body = self
            .call("/auth/token/create", vec![("code", code.to_owned())])
            .await?;
        Tokens::from_json(&body).ok_or_else(|| AeError::new("response", "no token in the answer"))
    }

    /// New tokens for the account, before the access token lapses.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<Tokens, AeError> {
        let body = self
            .call(
                "/auth/token/refresh",
                vec![("refresh_token", refresh_token.to_owned())],
            )
            .await?;
        Tokens::from_json(&body).ok_or_else(|| AeError::new("response", "no token in the answer"))
    }

    /// One product as a dropshipper buys it: shipped to the market's
    /// country, priced in its currency and worded in its language.
    pub async fn product(&self, product_id: &str, market: &Market) -> Result<AeProduct, AeError> {
        let body = self
            .call(
                "aliexpress.ds.product.get",
                vec![
                    ("product_id", product_id.to_owned()),
                    ("ship_to_country", market.ship_to_country.clone()),
                    ("target_currency", market.currency.clone()),
                    ("target_language", market.language.to_ascii_uppercase()),
                ],
            )
            .await?;
        AeProduct::from_response(product_id, &body)
    }

    /// What delivering `product` to the market's country costs and takes,
    /// and where from, asked for one combination
    /// ([`AeProduct::delivery_sku`]) with the service an order would ship
    /// with ([`choose_service`]); `None` for an item no longer sold, or with
    /// no combination to ask about. One call. Some sellers list an item at a
    /// few cents and charge its price as delivery: a cost without delivery
    /// would sell such an item at a loss ([`AeProduct::add_delivery`]).
    pub async fn delivery(
        &self,
        product: &AeProduct,
        market: &Market,
    ) -> Result<Option<Delivery>, AeError> {
        if !product.on_sale() {
            return Ok(None);
        }
        let Some(sku) = product.delivery_sku(&market.currency) else {
            return Ok(None);
        };
        let country = &market.ship_to_country;
        let options = self
            .freight(&product.id, &sku.id, 1, country, None, market)
            .await?;
        let Some(service) = choose_service(&options) else {
            let message = if options.is_empty() {
                format!("it delivers this item nowhere in {country}")
            } else {
                format!(
                    "no delivery to {country} can be followed: {}",
                    describe_options(&options, &market.currency)
                )
            };
            return Err(AeError::new("freight", message));
        };
        Ok(Some(Delivery {
            sku_id: sku.id.clone(),
            sku_cost_cents: sku.cost_cents.unwrap_or_default(),
            fee_cents: service.cents,
            days: service.delivery_days(),
            ships_from: service.ships_from.clone(),
        }))
    }

    /// AliExpress's places in `country`, `None` for a country without a list.
    pub async fn address_tree(&self, country: &str) -> Result<Option<AddressTree>, AeError> {
        let body = self
            .call_within(
                "aliexpress.ds.address.get",
                vec![("countryCode", country.to_owned())],
                Some(ADDRESS_TREE_TIMEOUT),
            )
            .await?;
        AddressTree::from_response(&body)
    }

    /// How AliExpress can deliver `quantity` of one combination to
    /// `country`. An item from a warehouse in that country needs `province`,
    /// but this call does not know every name the address form lists (`Gard`,
    /// in France), so one it refuses is left out.
    pub async fn freight(
        &self,
        product_id: &str,
        sku_id: &str,
        quantity: u32,
        country: &str,
        province: Option<&str>,
        market: &Market,
    ) -> Result<Vec<FreightOption>, AeError> {
        let answer = self
            .freight_to(product_id, sku_id, quantity, country, province, market)
            .await;
        match answer {
            Err(err) if province.is_some() && err.code.contains(INVALID_PROVINCE) => {
                tracing::info!(
                    country,
                    ?province,
                    "delivery asked again without the province"
                );
                self.freight_to(product_id, sku_id, quantity, country, None, market)
                    .await
            }
            answer => answer,
        }
    }

    async fn freight_to(
        &self,
        product_id: &str,
        sku_id: &str,
        quantity: u32,
        country: &str,
        province: Option<&str>,
        market: &Market,
    ) -> Result<Vec<FreightOption>, AeError> {
        let mut query = serde_json::json!({
            "productId": product_id,
            "selectedSkuId": sku_id,
            "quantity": quantity,
            "shipToCountry": country,
            "currency": market.currency,
            "language": market.language,
            "locale": format!("{}_{country}", market.language),
        });
        if let Some(province) = province {
            query["province"] = Value::from(province);
        }
        let answer = self
            .call(
                "aliexpress.ds.freight.query",
                vec![("queryDeliveryReq", query.to_string())],
            )
            .await;
        // "No way to deliver" reads the same whether AliExpress said so or its
        // answer could not be read, so the answer itself goes in the log.
        match answer {
            Ok(body) => {
                let options = freight_from_response(&body)?;
                if options.is_empty() {
                    tracing::warn!(product_id, sku_id, country, answer = %body, "no delivery offered");
                }
                Ok(options)
            }
            Err(err) if err.code.contains(NO_DELIVERY) || err.message.contains(NO_DELIVERY) => {
                tracing::warn!(product_id, sku_id, country, %err, "no delivery offered");
                Ok(Vec::new())
            }
            Err(err) => Err(err),
        }
    }

    /// Places an order for the account: the AliExpress order numbers, one
    /// per seller.
    pub async fn place_order(&self, order: &PlaceOrder) -> Result<Vec<String>, AeError> {
        let (request, extend) = order.params();
        let body = self
            .call(
                "aliexpress.ds.order.create",
                vec![
                    ("param_place_order_request4_open_api_d_t_o", request),
                    ("ds_extend_request", extend),
                ],
            )
            .await?;
        placed_orders_from_response(&body)
    }

    /// One order the account placed, as it now stands.
    pub async fn order(&self, ae_order_id: &str) -> Result<AeOrder, AeError> {
        let body = self
            .call(
                "aliexpress.trade.ds.order.get",
                vec![
                    ("order_id", ae_order_id.to_owned()),
                    (
                        "single_order_query",
                        serde_json::json!({ "order_id": ae_order_id }).to_string(),
                    ),
                ],
            )
            .await?;
        AeOrder::from_response(&body)
    }

    /// Where one parcel of an order the account placed is, step by step.
    pub async fn parcel_events(&self, parcel: &ParcelQuery) -> Result<Vec<Checkpoint>, AeError> {
        let body = self
            .call(
                "aliexpress.logistics.ds.trackinginfo.query",
                vec![
                    ("logistics_no", parcel.number.clone()),
                    ("origin", "ESCROW".to_owned()),
                    ("out_ref", parcel.ae_order_id.clone()),
                    ("service_name", parcel.service_name.clone()),
                    ("to_area", parcel.country.clone()),
                ],
            )
            .await?;
        parcel_events_from_response(&body)
    }

    /// Signs and sends one call; the answer's body, or why there is none.
    async fn call(&self, method: &str, params: Vec<(&str, String)>) -> Result<Value, AeError> {
        self.call_within(method, params, None).await
    }

    /// [`Self::call`], with longer than the client's usual time to answer.
    async fn call_within(
        &self,
        method: &str,
        params: Vec<(&str, String)>,
        timeout: Option<Duration>,
    ) -> Result<Value, AeError> {
        if let Some(calls) = &self.calls {
            calls.fetch_add(1, Ordering::Relaxed);
        }
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis())
            .unwrap_or_default();
        let mut all: BTreeMap<String, String> = params
            .into_iter()
            .filter(|(_, value)| !value.is_empty())
            .map(|(name, value)| (name.to_owned(), value))
            .collect();
        all.insert("app_key".into(), self.app_key.clone());
        all.insert("sign_method".into(), "sha256".into());
        all.insert("timestamp".into(), timestamp.to_string());
        all.insert("simplify".into(), "true".into());
        if let Some(session) = &self.session {
            all.insert("session".into(), session.clone());
        }
        let (url, path) = if method.starts_with('/') {
            (format!("{}/rest{method}", self.api_url), Some(method))
        } else {
            all.insert("method".into(), method.to_owned());
            (format!("{}/sync", self.api_url), None)
        };
        let signature = sign(&self.app_secret, path, &all);
        all.insert("sign".into(), signature);
        let mut request = self.http.post(&url).query(&all);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        let response = request
            .send()
            .await
            .map_err(|err| AeError::transient("network", with_causes(&err)))?;
        let status = response.status();
        if status.is_server_error() || status.as_u16() == 429 {
            return Err(AeError::transient(
                format!("http_{}", status.as_u16()),
                status.to_string(),
            ));
        }
        if !status.is_success() {
            return Err(AeError::new(
                format!("http_{}", status.as_u16()),
                status.to_string(),
            ));
        }
        let body: Value = response
            .json()
            .await
            .map_err(|err| AeError::transient("response", with_causes(&err)))?;
        if let Some(error) = body.get("error_response") {
            let code = error
                .get("code")
                .map(crate::model::text)
                .unwrap_or_default();
            let sub = error
                .get("sub_code")
                .map(crate::model::text)
                .unwrap_or_default();
            let message = error
                .get("msg")
                .or_else(|| error.get("sub_msg"))
                .map(crate::model::text)
                .unwrap_or_default();
            let code = if sub.is_empty() { code } else { sub };
            return Err(AeError::new(code, message));
        }
        // The calls addressed by path answer with a `code` of their own.
        if path.is_some()
            && let Some(code) = body.get("code").map(crate::model::text)
            && code != "0"
            && !code.is_empty()
        {
            let message = body
                .get("message")
                .or_else(|| body.get("msg"))
                .map(crate::model::text)
                .unwrap_or_default();
            return Err(AeError::new(code, message));
        }
        Ok(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_parcel_is_delivered_when_its_last_step_says_so() {
        let step = |description: &str| Checkpoint {
            at: "2026-11-09 08:00:00".into(),
            place: String::new(),
            description: description.into(),
        };
        assert!(!parcel_delivered(&[]));
        assert!(!parcel_delivered(&[step("Shipment accepted by carrier")]));
        assert!(!parcel_delivered(&[step("Out for delivery")]));
        assert!(parcel_delivered(&[
            step("Out for delivery"),
            step("Delivered")
        ]));
        assert!(parcel_delivered(&[step("Signed by recipient")]));
        assert!(parcel_delivered(&[step("Delivered to pickup point")]));
        assert!(!parcel_delivered(&[
            step("Delivered"),
            step("Returned to sender")
        ]));
        assert!(!parcel_delivered(&[step(
            "Not delivered: addressee absent"
        )]));
        assert!(!parcel_delivered(&[step("Delivery failed")]));
    }

    #[test]
    fn parcel_events_are_read_leniently_and_a_refusal_is_an_error() {
        let body = serde_json::json!({
            "aliexpress_logistics_ds_trackinginfo_query_response": {
                "result_success": true,
                "details": { "details": [
                    { "event_desc": "Shipment accepted by carrier", "address": "Shenzhen", "event_date": "2026-11-08 10:12:00" },
                    { "event_desc": "", "address": "", "event_date": "" },
                    { "event_desc": "Departed from origin country", "event_date": "2026-11-09 08:00:00" },
                ] },
            },
        });
        let steps = parcel_events_from_response(&body).unwrap();
        assert_eq!(
            steps,
            vec![
                Checkpoint {
                    at: "2026-11-08 10:12:00".into(),
                    place: "Shenzhen".into(),
                    description: "Shipment accepted by carrier".into(),
                },
                Checkpoint {
                    at: "2026-11-09 08:00:00".into(),
                    place: String::new(),
                    description: "Departed from origin country".into(),
                },
            ]
        );
        // Nothing yet is no steps, not an error.
        let empty = serde_json::json!({
            "aliexpress_logistics_ds_trackinginfo_query_response": { "result_success": true },
        });
        assert_eq!(parcel_events_from_response(&empty).unwrap(), Vec::new());
        let refused = serde_json::json!({
            "aliexpress_logistics_ds_trackinginfo_query_response": {
                "result_success": false,
                "result_error": "NO_TRACKING",
                "error_desc": "no such parcel",
            },
        });
        let err = parcel_events_from_response(&refused).unwrap_err();
        assert_eq!(err.code, "NO_TRACKING");
        assert_eq!(err.message, "no such parcel");
    }
}
