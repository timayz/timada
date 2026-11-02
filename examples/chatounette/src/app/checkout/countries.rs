//! The countries the shop ships to, as the checkout's select lists them.

/// ISO 3166-1 alpha-2 and the name a shopper reads, in that order: the EU,
/// then its neighbours and a few more.
pub const COUNTRIES: &[(&str, &str)] = &[
    ("AT", "Autriche"),
    ("BE", "Belgique"),
    ("BG", "Bulgarie"),
    ("HR", "Croatie"),
    ("CY", "Chypre"),
    ("CZ", "Tchéquie"),
    ("DK", "Danemark"),
    ("EE", "Estonie"),
    ("FI", "Finlande"),
    ("FR", "France"),
    ("DE", "Allemagne"),
    ("GR", "Grèce"),
    ("HU", "Hongrie"),
    ("IE", "Irlande"),
    ("IT", "Italie"),
    ("LV", "Lettonie"),
    ("LT", "Lituanie"),
    ("LU", "Luxembourg"),
    ("MT", "Malte"),
    ("MC", "Monaco"),
    ("NL", "Pays-Bas"),
    ("PL", "Pologne"),
    ("PT", "Portugal"),
    ("RO", "Roumanie"),
    ("SK", "Slovaquie"),
    ("SI", "Slovénie"),
    ("ES", "Espagne"),
    ("SE", "Suède"),
    ("CA", "Canada"),
    ("NO", "Norvège"),
    ("CH", "Suisse"),
    ("GB", "Royaume-Uni"),
    ("US", "États-Unis"),
];

/// Where a checkout starts when the shopper has not said.
pub const DEFAULT_COUNTRY: &str = "FR";

/// Each country's calling code, in the order of [`COUNTRIES`].
pub const DIAL_CODES: &[(&str, &str)] = &[
    ("AT", "43"),
    ("BE", "32"),
    ("BG", "359"),
    ("HR", "385"),
    ("CY", "357"),
    ("CZ", "420"),
    ("DK", "45"),
    ("EE", "372"),
    ("FI", "358"),
    ("FR", "33"),
    ("DE", "49"),
    ("GR", "30"),
    ("HU", "36"),
    ("IE", "353"),
    ("IT", "39"),
    ("LV", "371"),
    ("LT", "370"),
    ("LU", "352"),
    ("MT", "356"),
    ("MC", "377"),
    ("NL", "31"),
    ("PL", "48"),
    ("PT", "351"),
    ("RO", "40"),
    ("SK", "421"),
    ("SI", "386"),
    ("ES", "34"),
    ("SE", "46"),
    ("CA", "1"),
    ("NO", "47"),
    ("CH", "41"),
    ("GB", "44"),
    ("US", "1"),
];

/// The calling code of a country the shop ships to.
pub fn dial_code_of(country: &str) -> Option<&'static str> {
    DIAL_CODES
        .iter()
        .find(|(code, _)| *code == country)
        .map(|(_, dial)| *dial)
}

/// The country a calling code stands for in the picker: `preferred` when it
/// has that code (Canada and the United States share `1`), else the first.
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
