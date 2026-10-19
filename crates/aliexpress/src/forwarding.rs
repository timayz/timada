//! What a shop's paid order becomes on AliExpress: the combinations it buys
//! there, the delivery service each one ships with, and the address, as
//! AliExpress names its parts.

use timada_orders::{OrderLine, ShippingAddress};

use crate::client::LogisticsAddress;
use crate::command::LinkState;
use crate::model::FreightOption;

/// One combination to buy on AliExpress for an order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedItem {
    pub ae_product_id: String,
    pub sku_id: String,
    pub quantity: u32,
}

/// What of an order AliExpress sells.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ForwardPlan {
    /// One per combination, the lines that buy it merged.
    pub items: Vec<PlannedItem>,
    /// Every line of the order is one of `items`.
    pub complete: bool,
}

/// The combinations an order's lines buy, given the shop's linked items
/// (`links`; unlinked ones are left out). A line of a product imported from
/// AliExpress is the combination its variant (or the product, when it does
/// not vary) was linked to; a variant the merchant added by hand is not one.
pub fn forward_items(lines: &[OrderLine], links: &[LinkState]) -> ForwardPlan {
    let mut plan = ForwardPlan {
        items: Vec::new(),
        complete: true,
    };
    for line in lines {
        let found = links
            .iter()
            .filter(|link| link.is_linked())
            .filter(|link| link.product_id.as_deref() == Some(line.product_id.as_str()))
            .find_map(|link| {
                link.skus
                    .iter()
                    .find(|sku| sku.variant_id == line.variant_id)
                    .map(|sku| (link, sku))
            });
        let Some((link, sku)) = found else {
            plan.complete = false;
            continue;
        };
        match plan
            .items
            .iter_mut()
            .find(|item| item.ae_product_id == link.ae_product_id && item.sku_id == sku.sku_id)
        {
            Some(item) => item.quantity = item.quantity.saturating_add(line.quantity),
            None => plan.items.push(PlannedItem {
                ae_product_id: link.ae_product_id.clone(),
                sku_id: sku.sku_id.clone(),
                quantity: line.quantity,
            }),
        }
    }
    if plan.items.is_empty() {
        plan.complete = false;
    }
    plan
}

/// Whether AliExpress itself ships with this service (Cainiao, its own
/// logistics), rather than the seller.
pub fn shipped_by_aliexpress(service_name: &str) -> bool {
    let name = service_name.to_ascii_uppercase();
    name.starts_with("CAINIAO") || name.starts_with("AE_") || name.starts_with("ALIEXPRESS")
}

/// The service an item ships with: free and shipped by AliExpress, tracked
/// first; `None` when AliExpress offers no such delivery.
pub fn choose_service(options: &[FreightOption]) -> Option<&FreightOption> {
    options
        .iter()
        .filter(|option| option.cents == 0 && shipped_by_aliexpress(&option.service_name))
        .min_by_key(|option| (!option.tracking, option.service_name.clone()))
}

/// The address of an order as AliExpress takes it; `language` (`fr`) is the
/// shopper's, for the courier's messages.
pub fn logistics_address(address: &ShippingAddress, language: Option<&str>) -> LogisticsAddress {
    let country = address.country.trim().to_ascii_uppercase();
    let (phone_country, mobile_no) = address
        .phone
        .as_deref()
        .map(|phone| split_phone(phone, &country))
        .unwrap_or_default();
    let locale = language
        .map(str::trim)
        .filter(|language| language.len() == 2)
        .map(|language| format!("{}_{country}", language.to_ascii_lowercase()))
        .unwrap_or_default();
    LogisticsAddress {
        full_name: address.name.trim().to_owned(),
        contact_person: address.name.trim().to_owned(),
        address: address.line1.trim().to_owned(),
        address2: address
            .line2
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_owned(),
        city: address.city.trim().to_owned(),
        zip: address.postal_code.trim().to_owned(),
        country,
        phone_country,
        mobile_no,
        locale,
    }
}

/// A phone as AliExpress wants it: the country's calling code (`+33`) and
/// the national number, digits only, without the trunk `0`. A number given
/// in international form for another country keeps that country's code.
pub fn split_phone(phone: &str, country: &str) -> (String, String) {
    let international = phone.trim_start().starts_with('+') || phone.trim_start().starts_with("00");
    let mut digits: String = phone.chars().filter(char::is_ascii_digit).collect();
    if phone.trim_start().starts_with("00") {
        digits.drain(..2);
    }
    if digits.is_empty() {
        return (String::new(), String::new());
    }
    let home = calling_code(country);
    if international {
        // The longest code the number starts with, the country's first.
        let code = home
            .filter(|code| digits.starts_with(code))
            .or_else(|| {
                (1..=3)
                    .rev()
                    .map(|len| &digits[..len.min(digits.len())])
                    .find(|prefix| CALLING_CODES.iter().any(|(_, code)| code == prefix))
            })
            .map(str::to_owned);
        return match code {
            Some(code) => {
                let national = digits[code.len()..].trim_start_matches('0').to_owned();
                (format!("+{code}"), national)
            }
            None => (String::new(), digits),
        };
    }
    let national = if country == "IT" {
        digits
    } else {
        digits.trim_start_matches('0').to_owned()
    };
    (
        home.map(|code| format!("+{code}")).unwrap_or_default(),
        national,
    )
}

/// The calling code of a country a shop ships to, without `+`.
pub fn calling_code(country: &str) -> Option<&'static str> {
    CALLING_CODES
        .iter()
        .find(|(iso, _)| iso.eq_ignore_ascii_case(country))
        .map(|(_, code)| *code)
}

const CALLING_CODES: &[(&str, &str)] = &[
    ("AD", "376"),
    ("AE", "971"),
    ("AT", "43"),
    ("AU", "61"),
    ("BE", "32"),
    ("BG", "359"),
    ("BR", "55"),
    ("CA", "1"),
    ("CH", "41"),
    ("CY", "357"),
    ("CZ", "420"),
    ("DE", "49"),
    ("DK", "45"),
    ("DZ", "213"),
    ("EE", "372"),
    ("ES", "34"),
    ("FI", "358"),
    ("FR", "33"),
    ("GB", "44"),
    ("GR", "30"),
    ("HR", "385"),
    ("HU", "36"),
    ("IE", "353"),
    ("IL", "972"),
    ("IS", "354"),
    ("IT", "39"),
    ("JP", "81"),
    ("KR", "82"),
    ("LI", "423"),
    ("LT", "370"),
    ("LU", "352"),
    ("LV", "371"),
    ("MA", "212"),
    ("MC", "377"),
    ("MT", "356"),
    ("MX", "52"),
    ("NL", "31"),
    ("NO", "47"),
    ("NZ", "64"),
    ("PL", "48"),
    ("PT", "351"),
    ("RO", "40"),
    ("SA", "966"),
    ("SE", "46"),
    ("SG", "65"),
    ("SI", "386"),
    ("SK", "421"),
    ("TN", "216"),
    ("TR", "90"),
    ("UA", "380"),
    ("US", "1"),
];
