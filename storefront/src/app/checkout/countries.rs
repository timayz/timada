//! The countries the shop ships to, as the checkout's select lists them.

use std::{borrow::Cow, sync::LazyLock};

use timada_storefront::ShippingOption;
use topcoat::{
    context::Cx,
    l10n::{Catalog, locale, try_locale},
};

/// The countries a form names first, by ISO 3166-1 alpha-2 code, in this
/// order: the EU, then its neighbours and a few more.
pub const COUNTRIES: &[&str] = &[
    "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT", "LV",
    "LT", "LU", "MT", "MC", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE", "CA", "NO", "CH", "GB",
    "US",
];

/// Every other country a shipping method can name, for those a shop ticks: its
/// shipping page offers them all.
const OTHER_COUNTRIES: &[&str] = &[
    "AF", "ZA", "AL", "DZ", "AD", "AO", "AI", "AQ", "AG", "SA", "AR", "AM", "AW", "AU", "AZ", "BS",
    "BH", "BD", "BB", "BZ", "BJ", "BM", "BT", "BY", "BO", "BA", "BW", "BR", "BN", "BF", "BI", "KH",
    "CM", "CV", "CL", "CN", "CO", "KM", "CG", "CD", "KP", "KR", "CR", "CI", "CU", "CW", "DJ", "DM",
    "EG", "AE", "EC", "ER", "SZ", "VA", "ET", "FJ", "GA", "GM", "GE", "GS", "GH", "GI", "GD", "GL",
    "GP", "GU", "GT", "GG", "GN", "GQ", "GW", "GY", "GF", "HT", "HN", "HK", "BV", "CX", "IM", "NF",
    "AX", "KY", "CC", "CK", "FO", "HM", "FK", "MP", "MH", "UM", "PN", "SB", "TC", "VG", "VI", "IN",
    "ID", "IQ", "IR", "IS", "IL", "JM", "JP", "JE", "JO", "KZ", "KE", "KG", "KI", "KW", "RE", "LA",
    "LS", "LB", "LR", "LY", "LI", "MO", "MK", "MG", "MY", "MW", "MV", "ML", "MA", "MQ", "MU", "MR",
    "YT", "MX", "FM", "MD", "MN", "ME", "MS", "MZ", "MM", "NA", "NR", "NP", "NI", "NE", "NG", "NU",
    "NC", "NZ", "OM", "UG", "UZ", "PK", "PW", "PS", "PA", "PG", "PY", "BQ", "PE", "PH", "PF", "PR",
    "QA", "CF", "DO", "RU", "RW", "EH", "BL", "KN", "SM", "MF", "SX", "PM", "VC", "SH", "LC", "SV",
    "WS", "AS", "ST", "SN", "RS", "SC", "SL", "SG", "SO", "SD", "SS", "LK", "SR", "SJ", "SY", "TJ",
    "TW", "TZ", "TD", "TF", "IO", "TH", "TL", "TG", "TK", "TO", "TT", "TN", "TM", "TR", "TV", "UA",
    "UY", "VU", "VE", "VN", "WF", "YE", "ZM", "ZW",
];

/// The countries a form can name for a shop with these shipping methods: the
/// usual ones, then any other that a method names. One that ships everywhere
/// keeps to the usual ones.
pub fn countries_for(methods: &[ShippingOption]) -> impl Iterator<Item = &'static str> + '_ {
    COUNTRIES
        .iter()
        .chain(OTHER_COUNTRIES.iter().filter(|code| {
            methods
                .iter()
                .any(|method| method.countries.iter().any(|c| c == *code))
        }))
        .copied()
}

/// The names of the countries in each language the storefront speaks, as
/// Fluent files beside this one (`countries/fr.ftl`), keyed by country
/// code: a list read by code, apart from the sentences of the pages.
static NAMES: LazyLock<Catalog> = LazyLock::new(|| {
    Catalog::new([
        (locale!("fr"), &[include_str!("countries/fr.ftl")][..]),
        (locale!("en"), &[include_str!("countries/en.ftl")][..]),
    ])
});

/// What a shopper reads for the country `code`, in the language of the
/// request: its code, for one no file names.
pub fn country_name(cx: &Cx, code: &'static str) -> Cow<'static, str> {
    try_locale(cx)
        .and_then(|locale| NAMES.format(locale, code, None, &[]))
        .unwrap_or(Cow::Borrowed(code))
}

/// Where a checkout starts when the shopper has not said.
pub const DEFAULT_COUNTRY: &str = "FR";

/// Every country's calling code, by code, the country a shared code is
/// mainly dialled for first (`+1` is the United States before Canada and the
/// Caribbean): the first with a code is the one a whole number is taken to
/// come from.
pub const DIAL_CODES: &[(&str, &str)] = &[
    ("US", "1"),
    ("AG", "1"),
    ("AI", "1"),
    ("AS", "1"),
    ("BB", "1"),
    ("BM", "1"),
    ("BS", "1"),
    ("CA", "1"),
    ("DM", "1"),
    ("DO", "1"),
    ("GD", "1"),
    ("GU", "1"),
    ("JM", "1"),
    ("KN", "1"),
    ("KY", "1"),
    ("LC", "1"),
    ("MP", "1"),
    ("MS", "1"),
    ("PR", "1"),
    ("SX", "1"),
    ("TC", "1"),
    ("TT", "1"),
    ("VC", "1"),
    ("VG", "1"),
    ("VI", "1"),
    ("RU", "7"),
    ("KZ", "7"),
    ("EG", "20"),
    ("ZA", "27"),
    ("GR", "30"),
    ("NL", "31"),
    ("BE", "32"),
    ("FR", "33"),
    ("ES", "34"),
    ("HU", "36"),
    ("IT", "39"),
    ("VA", "39"),
    ("RO", "40"),
    ("CH", "41"),
    ("AT", "43"),
    ("GB", "44"),
    ("GG", "44"),
    ("IM", "44"),
    ("JE", "44"),
    ("DK", "45"),
    ("SE", "46"),
    ("NO", "47"),
    ("SJ", "47"),
    ("PL", "48"),
    ("DE", "49"),
    ("PE", "51"),
    ("MX", "52"),
    ("CU", "53"),
    ("AR", "54"),
    ("BR", "55"),
    ("CL", "56"),
    ("CO", "57"),
    ("VE", "58"),
    ("MY", "60"),
    ("AU", "61"),
    ("CC", "61"),
    ("CX", "61"),
    ("ID", "62"),
    ("PH", "63"),
    ("NZ", "64"),
    ("SG", "65"),
    ("TH", "66"),
    ("JP", "81"),
    ("KR", "82"),
    ("VN", "84"),
    ("CN", "86"),
    ("TR", "90"),
    ("IN", "91"),
    ("PK", "92"),
    ("AF", "93"),
    ("LK", "94"),
    ("MM", "95"),
    ("IR", "98"),
    ("SS", "211"),
    ("MA", "212"),
    ("EH", "212"),
    ("DZ", "213"),
    ("TN", "216"),
    ("LY", "218"),
    ("GM", "220"),
    ("SN", "221"),
    ("MR", "222"),
    ("ML", "223"),
    ("GN", "224"),
    ("CI", "225"),
    ("BF", "226"),
    ("NE", "227"),
    ("TG", "228"),
    ("BJ", "229"),
    ("MU", "230"),
    ("LR", "231"),
    ("SL", "232"),
    ("GH", "233"),
    ("NG", "234"),
    ("TD", "235"),
    ("CF", "236"),
    ("CM", "237"),
    ("CV", "238"),
    ("ST", "239"),
    ("GQ", "240"),
    ("GA", "241"),
    ("CG", "242"),
    ("CD", "243"),
    ("AO", "244"),
    ("GW", "245"),
    ("IO", "246"),
    ("SC", "248"),
    ("SD", "249"),
    ("RW", "250"),
    ("ET", "251"),
    ("SO", "252"),
    ("DJ", "253"),
    ("KE", "254"),
    ("TZ", "255"),
    ("UG", "256"),
    ("BI", "257"),
    ("MZ", "258"),
    ("ZM", "260"),
    ("MG", "261"),
    ("RE", "262"),
    ("YT", "262"),
    ("ZW", "263"),
    ("NA", "264"),
    ("MW", "265"),
    ("LS", "266"),
    ("BW", "267"),
    ("SZ", "268"),
    ("KM", "269"),
    ("SH", "290"),
    ("ER", "291"),
    ("AW", "297"),
    ("FO", "298"),
    ("GL", "299"),
    ("GI", "350"),
    ("PT", "351"),
    ("LU", "352"),
    ("IE", "353"),
    ("IS", "354"),
    ("AL", "355"),
    ("MT", "356"),
    ("CY", "357"),
    ("FI", "358"),
    ("AX", "358"),
    ("BG", "359"),
    ("LT", "370"),
    ("LV", "371"),
    ("EE", "372"),
    ("MD", "373"),
    ("AM", "374"),
    ("BY", "375"),
    ("AD", "376"),
    ("MC", "377"),
    ("SM", "378"),
    ("UA", "380"),
    ("RS", "381"),
    ("ME", "382"),
    ("XK", "383"),
    ("HR", "385"),
    ("SI", "386"),
    ("BA", "387"),
    ("MK", "389"),
    ("CZ", "420"),
    ("SK", "421"),
    ("LI", "423"),
    ("FK", "500"),
    ("BZ", "501"),
    ("GT", "502"),
    ("SV", "503"),
    ("HN", "504"),
    ("NI", "505"),
    ("CR", "506"),
    ("PA", "507"),
    ("PM", "508"),
    ("HT", "509"),
    ("GP", "590"),
    ("BL", "590"),
    ("MF", "590"),
    ("BO", "591"),
    ("GY", "592"),
    ("EC", "593"),
    ("GF", "594"),
    ("PY", "595"),
    ("MQ", "596"),
    ("SR", "597"),
    ("UY", "598"),
    ("CW", "599"),
    ("BQ", "599"),
    ("TL", "670"),
    ("NF", "672"),
    ("BN", "673"),
    ("NR", "674"),
    ("PG", "675"),
    ("TO", "676"),
    ("SB", "677"),
    ("VU", "678"),
    ("FJ", "679"),
    ("PW", "680"),
    ("WF", "681"),
    ("CK", "682"),
    ("NU", "683"),
    ("WS", "685"),
    ("KI", "686"),
    ("NC", "687"),
    ("TV", "688"),
    ("PF", "689"),
    ("TK", "690"),
    ("FM", "691"),
    ("MH", "692"),
    ("KP", "850"),
    ("HK", "852"),
    ("MO", "853"),
    ("KH", "855"),
    ("LA", "856"),
    ("BD", "880"),
    ("TW", "886"),
    ("MV", "960"),
    ("LB", "961"),
    ("JO", "962"),
    ("SY", "963"),
    ("IQ", "964"),
    ("KW", "965"),
    ("SA", "966"),
    ("YE", "967"),
    ("OM", "968"),
    ("PS", "970"),
    ("AE", "971"),
    ("IL", "972"),
    ("BH", "973"),
    ("QA", "974"),
    ("BT", "975"),
    ("MN", "976"),
    ("NP", "977"),
    ("TJ", "992"),
    ("TM", "993"),
    ("AZ", "994"),
    ("GE", "995"),
    ("KG", "996"),
    ("UZ", "998"),
];

/// A country's calling code.
pub fn dial_code_of(country: &str) -> Option<&'static str> {
    DIAL_CODES
        .iter()
        .find(|(code, _)| *code == country)
        .map(|(_, dial)| *dial)
}

/// The country a calling code stands for in the picker: `preferred` when it
/// has that code (Canada and the United States share `1`), else the one it
/// is mainly dialled for.
pub fn country_of_dial(dial: &str, preferred: &str) -> Option<&'static str> {
    DIAL_CODES
        .iter()
        .find(|(country, code)| *code == dial && *country == preferred)
        .or_else(|| DIAL_CODES.iter().find(|(_, code)| *code == dial))
        .map(|(country, _)| *country)
}

/// A country's flag, as the two regional indicator letters an emoji font
/// draws as one.
pub fn flag(country: &str) -> String {
    country
        .chars()
        .filter_map(|c| {
            char::from_u32(0x1F1E6 + u32::from(c.to_ascii_uppercase()) - u32::from('A'))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_country_a_shipping_method_can_name_is_listed_once() {
        for country in rust_iso3166::ALL {
            let named = COUNTRIES
                .iter()
                .chain(OTHER_COUNTRIES)
                .filter(|code| **code == country.alpha2)
                .count();
            assert_eq!(named, 1, "{}", country.alpha2);
        }
    }

    #[test]
    fn every_country_has_a_name_in_each_language() {
        for language in crate::app::lang::SPOKEN {
            let cx = Cx::new().with(language.parse::<topcoat::l10n::Locale>().unwrap());
            for code in COUNTRIES.iter().chain(OTHER_COUNTRIES) {
                assert_ne!(country_name(&cx, code), *code, "{code} in {language}");
            }
        }
    }
}
