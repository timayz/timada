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

/// The service an item ships with: one AliExpress itself ships ("Shipped by
/// AliExpress"), free before paid, tracked before not, the cheapest paid one
/// otherwise; `None` when only the seller delivers.
pub fn choose_service(options: &[FreightOption]) -> Option<&FreightOption> {
    options
        .iter()
        .filter(|option| shipped_by_aliexpress(&option.service_name))
        .min_by_key(|option| (option.cents, !option.tracking, option.service_name.clone()))
}

/// The delivery services offered, for the merchant to read:
/// `CAINIAO_PREMIUM (9.90 EUR, tracked), YANWEN_JYT (free)`.
pub fn describe_options(options: &[FreightOption], currency: &str) -> String {
    options
        .iter()
        .map(|option| {
            let fee = if option.cents == 0 {
                "free".to_owned()
            } else {
                format!(
                    "{}.{:02} {currency}",
                    option.cents / 100,
                    option.cents % 100
                )
            };
            let tracked = if option.tracking { ", tracked" } else { "" };
            format!("{} ({fee}{tracked})", option.service_name)
        })
        .collect::<Vec<_>>()
        .join(", ")
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
        province: province_of(&country, &address.postal_code, &address.city),
        zip: address.postal_code.trim().to_owned(),
        country,
        phone_country,
        mobile_no,
        locale,
    }
}

/// The region AliExpress files an address under, which it will not do
/// without: the French region of a French postal code, the city otherwise
/// (the shop's addresses carry no region of their own).
pub fn province_of(country: &str, postal_code: &str, city: &str) -> String {
    let region = match country {
        "FR" => french_region(postal_code),
        _ => None,
    };
    region.map_or_else(|| city.trim().to_owned(), str::to_owned)
}

/// The region of a French postal code, named as AliExpress lists the
/// regions of France (the 22 of before 2016, unaccented), by its department;
/// `None` for a postal code that is not French.
pub fn french_region(postal_code: &str) -> Option<&'static str> {
    let digits: String = postal_code.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 5 {
        return None;
    }
    if let Some(overseas) = match &digits[..3] {
        "971" => Some("Guadeloupe"),
        "972" => Some("Martinique"),
        "973" => Some("Guyane"),
        "974" => Some("Reunion"),
        "975" => Some("Saint-Pierre-et-Miquelon"),
        "976" => Some("Mayotte"),
        "977" => Some("Saint-Barthelemy"),
        "978" => Some("Saint-Martin"),
        "986" => Some("Wallis-et-Futuna"),
        "987" => Some("Polynesie Francaise"),
        "988" => Some("Nouvelle-Caledonie"),
        _ => None,
    } {
        return Some(overseas);
    }
    Some(match &digits[..2] {
        "67" | "68" => "Alsace",
        "24" | "33" | "40" | "47" | "64" => "Aquitaine",
        "03" | "15" | "43" | "63" => "Auvergne",
        "14" | "50" | "61" => "Basse-Normandie",
        "21" | "58" | "71" | "89" => "Bourgogne",
        "22" | "29" | "35" | "56" => "Bretagne",
        "18" | "28" | "36" | "37" | "41" | "45" => "Centre",
        "08" | "10" | "51" | "52" => "Champagne-Ardenne",
        "20" => "Corse",
        "25" | "39" | "70" | "90" => "Franche-Comte",
        "27" | "76" => "Haute-Normandie",
        "75" | "77" | "78" | "91" | "92" | "93" | "94" | "95" => "Ile-de-France",
        "11" | "30" | "34" | "48" | "66" => "Languedoc-Roussillon",
        "19" | "23" | "87" => "Limousin",
        "54" | "55" | "57" | "88" => "Lorraine",
        "09" | "12" | "31" | "32" | "46" | "65" | "81" | "82" => "Midi-Pyrenees",
        "59" | "62" => "Nord-Pas-de-Calais",
        "44" | "49" | "53" | "72" | "85" => "Pays de la Loire",
        "02" | "60" | "80" => "Picardie",
        "16" | "17" | "79" | "86" => "Poitou-Charentes",
        "04" | "05" | "06" | "13" | "83" | "84" => "Provence-Alpes-Cote d'Azur",
        "01" | "07" | "26" | "38" | "42" | "69" | "73" | "74" => "Rhone-Alpes",
        _ => return None,
    })
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
