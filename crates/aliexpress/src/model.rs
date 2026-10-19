//! What AliExpress answers, read leniently: numbers come as strings or
//! numbers, a list comes bare or wrapped in an object of one key
//! (`{"ae_item_sku_info_d_t_o": [...]}`), and a field may be missing or
//! `null`. Anything unreadable is left out rather than failing the product.

use serde_json::Value;

/// Where and how the shop buys: the country goods ship to, the currency it
/// is charged in and the language products are worded in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Market {
    /// ISO 3166 alpha-2 (`FR`).
    pub ship_to_country: String,
    /// ISO 4217 (`EUR`).
    pub currency: String,
    /// ISO 639-1 (`fr`).
    pub language: String,
}

/// The tokens an account gave the app.
#[derive(Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: String,
    /// Seconds from now.
    pub expires_in: u64,
    pub refresh_expires_in: u64,
    /// The account's name, as AliExpress shows it.
    pub account: String,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tokens")
            .field("expires_in", &self.expires_in)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

impl Tokens {
    pub(crate) fn from_json(body: &Value) -> Option<Self> {
        let access_token = text(body.get("access_token")?);
        if access_token.is_empty() {
            return None;
        }
        Some(Self {
            access_token,
            refresh_token: body.get("refresh_token").map(text).unwrap_or_default(),
            expires_in: body.get("expires_in").and_then(number).unwrap_or(0),
            refresh_expires_in: body.get("refresh_expires_in").and_then(number).unwrap_or(0),
            account: ["account", "user_nick", "seller_id", "user_id"]
                .iter()
                .filter_map(|key| body.get(*key).map(text))
                .find(|value| !value.is_empty())
                .unwrap_or_default(),
        })
    }
}

/// One product as AliExpress sells it to a dropshipper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AeProduct {
    pub id: String,
    pub subject: String,
    /// HTML.
    pub detail: String,
    pub currency: String,
    /// `onSelling` while it is sold.
    pub status: String,
    pub images: Vec<String>,
    pub skus: Vec<AeSku>,
    /// Its facts, in AliExpress's order.
    pub properties: Vec<(String, String)>,
}

/// One combination AliExpress sells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AeSku {
    pub id: String,
    /// What it costs the shop, in minor units; `None` when unpriced.
    pub cost_cents: Option<u64>,
    pub available: u32,
    /// The values it takes, in AliExpress's order.
    pub properties: Vec<AeSkuProperty>,
    /// How an order names it (`14:193#Red;5:100014064`), when given.
    pub attr: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AeSkuProperty {
    pub name: String,
    pub value: String,
    pub image: Option<String>,
}

impl AeProduct {
    /// Whether AliExpress still sells it.
    pub fn on_sale(&self) -> bool {
        self.status.is_empty() || self.status.eq_ignore_ascii_case("onSelling")
    }

    /// The combination with that id.
    pub fn sku(&self, sku_id: &str) -> Option<&AeSku> {
        self.skus.iter().find(|sku| sku.id == sku_id)
    }

    pub(crate) fn from_json(product_id: &str, result: &Value) -> Option<Self> {
        let base = result.get("ae_item_base_info_dto")?;
        let images = result
            .get("ae_multimedia_info_dto")
            .and_then(|media| media.get("image_urls"))
            .map(text)
            .unwrap_or_default()
            .split(';')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .collect();
        let skus = list(result.get("ae_item_sku_info_dtos"))
            .iter()
            .filter_map(AeSku::from_json)
            .collect();
        let properties = list(result.get("ae_item_properties"))
            .iter()
            .filter_map(|property| {
                let name = text(property.get("attr_name")?);
                let value = text(property.get("attr_value")?);
                (!name.is_empty() && !value.is_empty()).then_some((name, value))
            })
            .collect();
        Some(Self {
            id: base
                .get("product_id")
                .map(text)
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| product_id.to_owned()),
            subject: base.get("subject").map(text).unwrap_or_default(),
            detail: base
                .get("detail")
                .or_else(|| base.get("mobile_detail"))
                .map(text)
                .unwrap_or_default(),
            currency: base.get("currency_code").map(text).unwrap_or_default(),
            status: base
                .get("product_status_type")
                .map(text)
                .unwrap_or_default(),
            images,
            skus,
            properties,
        })
    }
}

impl AeSku {
    fn from_json(sku: &Value) -> Option<Self> {
        let id = ["sku_id", "id"]
            .iter()
            .filter_map(|key| sku.get(*key).map(text))
            .find(|id| !id.is_empty())?;
        let cost_cents = ["offer_sale_price", "sku_price"]
            .iter()
            .filter_map(|key| sku.get(*key).and_then(cents))
            .find(|cents| *cents > 0);
        let in_stock = sku
            .get("sku_stock")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let available = [
            "sku_available_stock",
            "s_k_u_available_stock",
            "ipm_sku_stock",
        ]
        .iter()
        .filter_map(|key| sku.get(*key).and_then(number))
        .next()
        .unwrap_or(0);
        let available = if in_stock {
            u32::try_from(available).unwrap_or(u32::MAX)
        } else {
            0
        };
        let wrapped = sku
            .get("ae_sku_property_dtos")
            .or_else(|| sku.get("aeop_s_k_u_propertys"));
        let properties = list(wrapped)
            .iter()
            .filter_map(|property| {
                let name = text(property.get("sku_property_name")?);
                let value = property
                    .get("property_value_definition_name")
                    .map(text)
                    .filter(|value| !value.is_empty())
                    .or_else(|| property.get("sku_property_value").map(text))?;
                let image = property
                    .get("sku_image")
                    .map(text)
                    .filter(|url| !url.is_empty());
                (!name.is_empty() && !value.is_empty()).then_some(AeSkuProperty {
                    name,
                    value,
                    image,
                })
            })
            .collect();
        let attr = sku
            .get("sku_attr")
            .map(text)
            .filter(|attr| !attr.is_empty());
        Some(Self {
            id,
            cost_cents,
            available,
            properties,
            attr,
        })
    }
}

/// One way AliExpress delivers an item to a country.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FreightOption {
    /// What an order names it by (`CAINIAO_STANDARD`).
    pub service_name: String,
    /// What delivery costs, in minor units; 0 when free.
    pub cents: u64,
    /// Whether the parcel can be followed.
    pub tracking: bool,
}

impl FreightOption {
    pub(crate) fn list_from_json(result: &Value) -> Vec<Self> {
        let options = result
            .get("aeop_freight_calculate_result_for_buyer_d_t_o_list")
            .or_else(|| result.get("aeop_freight_calculate_result_for_buyer_dtolist"));
        list(options)
            .iter()
            .filter_map(|option| {
                let service_name = option.get("service_name").map(text)?;
                if service_name.is_empty() {
                    return None;
                }
                let freight = option.get("freight");
                let cents = freight
                    .and_then(|freight| {
                        freight
                            .get("cent")
                            .and_then(number)
                            .or_else(|| freight.get("amount").and_then(cents))
                    })
                    .unwrap_or(0);
                let tracking = option
                    .get("tracking_available")
                    .map(text)
                    .is_some_and(|flag| flag.eq_ignore_ascii_case("true"));
                Some(Self {
                    service_name,
                    cents,
                    tracking,
                })
            })
            .collect()
    }
}

/// One order placed on AliExpress, as it now stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AeOrder {
    /// `PLACE_ORDER_SUCCESS` (awaiting payment), `WAIT_SELLER_SEND_GOODS`,
    /// `WAIT_BUYER_ACCEPT_GOODS`, `FINISH`…; kept as AliExpress says it.
    pub status: String,
    /// `NO_LOGISTICS`, `WAIT_SELLER_SEND_GOODS`, `SELLER_SEND_GOODS`…
    pub logistics_status: String,
    /// The parcel numbers, once shipped.
    pub tracking: Vec<String>,
}

impl AeOrder {
    pub(crate) fn from_json(result: &Value) -> Self {
        let mut tracking: Vec<String> = list(result.get("logistics_info_list"))
            .iter()
            .chain(
                result
                    .get("logistics_info_list")
                    .filter(|value| value.get("logistics_no").is_some()),
            )
            .filter_map(|info| info.get("logistics_no").map(text))
            .filter(|number| !number.is_empty())
            .collect();
        let mut seen = Vec::new();
        tracking.retain(|number| {
            let first = !seen.contains(number);
            seen.push(number.clone());
            first
        });
        Self {
            status: result.get("order_status").map(text).unwrap_or_default(),
            logistics_status: result.get("logistics_status").map(text).unwrap_or_default(),
            tracking,
        }
    }
}

/// A field as text: a string as is, a number written out, anything else
/// empty.
pub(crate) fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.trim().to_owned(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        _ => String::new(),
    }
}

/// A whole number, given as a number or a string.
fn number(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number
            .as_u64()
            .or_else(|| number.as_f64().map(|f| f.max(0.0) as u64)),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// An amount (`"12.34"`, `12.34`) in minor units.
pub(crate) fn cents(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_f64().map(|f| (f.max(0.0) * 100.0).round() as u64),
        Value::String(text) => parse_amount(text),
        _ => None,
    }
}

/// `"12.34"` → 1234; `"12"` → 1200; `"12.345"` → 1235 (rounded).
pub fn parse_amount(text: &str) -> Option<u64> {
    let text = text.trim();
    let (whole, fraction) = text.split_once('.').unwrap_or((text, ""));
    if whole.is_empty() && fraction.is_empty() {
        return None;
    }
    let whole: u64 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut digits: Vec<u64> = fraction.bytes().map(|b| u64::from(b - b'0')).collect();
    digits.resize(3, 0);
    let mut cents = digits[0] * 10 + digits[1];
    if digits[2] >= 5 {
        cents += 1;
    }
    whole.checked_mul(100)?.checked_add(cents)
}

/// A list, bare or wrapped in an object of one key; a list of one may come
/// as that one item, wrapped or not.
pub(crate) fn list(value: Option<&Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items.clone(),
        Some(Value::Object(object)) => {
            if let Some(items) = object.values().find_map(|inner| inner.as_array()) {
                return items.clone();
            }
            match object.values().next() {
                Some(inner @ Value::Object(_)) if object.len() == 1 => vec![inner.clone()],
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amounts_read_as_minor_units() {
        assert_eq!(parse_amount("12.34"), Some(1234));
        assert_eq!(parse_amount("12"), Some(1200));
        assert_eq!(parse_amount("12.3"), Some(1230));
        assert_eq!(parse_amount("12.345"), Some(1235));
        assert_eq!(parse_amount("0.99"), Some(99));
        assert_eq!(parse_amount("abc"), None);
        assert_eq!(parse_amount(""), None);
    }
}
