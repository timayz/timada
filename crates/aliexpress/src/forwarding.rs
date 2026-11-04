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
/// shopper's, for the courier's messages. AliExpress wants a phone: the
/// shopper's, or `shop_phone` when they gave none.
pub fn logistics_address(
    address: &ShippingAddress,
    language: Option<&str>,
    shop_phone: Option<&str>,
) -> LogisticsAddress {
    let shop_country = address.country.trim().to_ascii_uppercase();
    let country = delivery_country(&shop_country, &address.postal_code);
    let (phone_country, mobile_no) = address
        .phone
        .as_deref()
        .filter(|phone| !phone.trim().is_empty())
        .or(shop_phone)
        .map(|phone| split_phone(phone, &country))
        .unwrap_or_default();
    let locale = language
        .map(str::trim)
        .filter(|language| language.len() == 2)
        .map(|language| format!("{}_{shop_country}", language.to_ascii_lowercase()))
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

/// The country AliExpress delivers an address to. The shop's checkout files a
/// French overseas department under France, AliExpress lists it as a country
/// of its own. Saint-Barthélemy (97133) and Saint-Martin (97150) share
/// Guadeloupe's prefix but are not part of it, so they stay French.
pub fn delivery_country(country: &str, postal_code: &str) -> String {
    let country = country.trim().to_ascii_uppercase();
    if country != "FR" {
        return country;
    }
    let digits: String = postal_code.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 5 {
        return country;
    }
    match &digits[..3] {
        "971" if !matches!(digits.as_str(), "97133" | "97150") => "GP",
        "972" => "MQ",
        "973" => "GF",
        "974" => "RE",
        "976" => "YT",
        _ => "FR",
    }
    .to_owned()
}

/// The region AliExpress files an address under, which it will not do
/// without: for France the department of the postal code, as AliExpress
/// lists them, or its "Other" for one it does not list; the city elsewhere
/// (the shop's addresses carry no region of their own, and Martinique's is a
/// free text there).
pub fn province_of(country: &str, postal_code: &str, city: &str) -> String {
    match country {
        "FR" => french_department(postal_code).unwrap_or("Other").to_owned(),
        _ => city.trim().to_owned(),
    }
}

/// The department of a French postal code, named as AliExpress lists the
/// departments of France (unaccented, metropolitan only: an overseas one is
/// not listed); `None` for a postal code that names none.
pub fn french_department(postal_code: &str) -> Option<&'static str> {
    let digits: String = postal_code.chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 5 {
        return None;
    }
    // Corsica: 200xx-201xx is Corse-du-Sud, 202xx-206xx Haute-Corse.
    if let Some(corsica) = match &digits[..3] {
        "200" | "201" => Some("Corse-du-Sud"),
        "202" | "203" | "204" | "205" | "206" => Some("Haute-Corse"),
        _ => None,
    } {
        return Some(corsica);
    }
    Some(match &digits[..2] {
        "01" => "Ain",
        "02" => "Aisne",
        "03" => "Allier",
        "04" => "Alpes-de-Haute-Provence",
        "05" => "Hautes-Alpes",
        "06" => "Alpes-Maritimes",
        "07" => "Ardeche",
        "08" => "Ardennes",
        "09" => "Ariege",
        "10" => "Aube",
        "11" => "Aude",
        "12" => "Aveyron",
        "13" => "Bouches-du-Rhone",
        "14" => "Calvados",
        "15" => "Cantal",
        "16" => "Charente",
        "17" => "Charente-Maritime",
        "18" => "Cher",
        "19" => "Correze",
        "21" => "Cote-d'Or",
        "22" => "Cotes-d'Armor",
        "23" => "Creuse",
        "24" => "Dordogne",
        "25" => "Doubs",
        "26" => "Drome",
        "27" => "Eure",
        "28" => "Eure-et-Loir",
        "29" => "Finistere",
        "30" => "Gard",
        "31" => "Haute-Garonne",
        "32" => "Gers",
        "33" => "Gironde",
        "34" => "Herault",
        "35" => "Ille-et-Vilaine",
        "36" => "Indre",
        "37" => "Indre-et-Loire",
        "38" => "Isere",
        "39" => "Jura",
        "40" => "Landes",
        "41" => "Loir-et-Cher",
        "42" => "Loire",
        "43" => "Haute-Loire",
        "44" => "Loire-Atlantique",
        "45" => "Loiret",
        "46" => "Lot",
        "47" => "Lot-et-Garonne",
        "48" => "Lozere",
        "49" => "Maine-et-Loire",
        "50" => "Manche",
        "51" => "Marne",
        "52" => "Haute-Marne",
        "53" => "Mayenne",
        "54" => "Meurthe-et-Moselle",
        "55" => "Meuse",
        "56" => "Morbihan",
        "57" => "Moselle",
        "58" => "Nievre",
        "59" => "Nord",
        "60" => "Oise",
        "61" => "Orne",
        "62" => "Pas-de-Calais",
        "63" => "Puy-de-Dome",
        "64" => "Pyrenees-Atlantiques",
        "65" => "Hautes-Pyrenees",
        "66" => "Pyrenees-Orientales",
        "67" => "Bas-Rhin",
        "68" => "Haut-Rhin",
        "69" => "Rhone",
        "70" => "Haute-Saone",
        "71" => "Saone-et-Loire",
        "72" => "Sarthe",
        "73" => "Savoie",
        "74" => "Haute-Savoie",
        "75" => "Paris",
        "76" => "Seine-Maritime",
        "77" => "Seine-et-Marne",
        "78" => "Yvelines",
        "79" => "Deux-Sevres",
        "80" => "Somme",
        "81" => "Tarn",
        "82" => "Tarn-et-Garonne",
        "83" => "Var",
        "84" => "Vaucluse",
        "85" => "Vendee",
        "86" => "Vienne",
        "87" => "Haute-Vienne",
        "88" => "Vosges",
        "89" => "Yonne",
        "90" => "Territoire de Belfort",
        "91" => "Essonne",
        "92" => "Hauts-de-Seine",
        "93" => "Seine-Saint-Denis",
        "94" => "Val-de-Marne",
        "95" => "Val-d'Oise",
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
    ("GF", "594"),
    ("GP", "590"),
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
    ("MQ", "596"),
    ("MT", "356"),
    ("MX", "52"),
    ("NL", "31"),
    ("NO", "47"),
    ("NZ", "64"),
    ("PL", "48"),
    ("PT", "351"),
    ("RE", "262"),
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
    ("YT", "262"),
];
