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
