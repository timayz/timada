use bitcode::{Decode, Encode};
use timada_core::Money;

/// How a sale is taxed. Persisted in `OrderTaxed` / `InvoiceTaxed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Encode, Decode)]
pub enum TaxTreatment {
    /// The shop's own VAT, as included in the listed price.
    #[default]
    Domestic,
    /// No VAT: the pre-tax price is charged.
    Export,
    /// The destination's VAT on top of the pre-tax price.
    DestinationVat,
}

impl TaxTreatment {
    pub fn as_str(self) -> &'static str {
        match self {
            TaxTreatment::Domestic => "domestic",
            TaxTreatment::Export => "export",
            TaxTreatment::DestinationVat => "destination-vat",
        }
    }

    /// The legal mention a document without VAT must carry.
    pub fn exemption_mention(self) -> Option<&'static str> {
        match self {
            TaxTreatment::Export => Some(
                "Exonération de TVA — articles 262 I et 294 du CGI (livraison hors du territoire fiscal)",
            ),
            TaxTreatment::Domestic | TaxTreatment::DestinationVat => None,
        }
    }

    /// What a document says about the VAT regime of the sale: the exemption
    /// of an export, or whose VAT a distance sale inside the EU carries.
    pub fn regime_mention(self) -> Option<&'static str> {
        match self {
            TaxTreatment::DestinationVat => Some(
                "TVA de l'État membre de livraison — vente à distance intracommunautaire, article 258 A du CGI (guichet unique OSS)",
            ),
            TaxTreatment::Domestic | TaxTreatment::Export => self.exemption_mention(),
        }
    }
}

/// The VAT of a destination country, for [`TaxTreatment::DestinationVat`].
///
/// A product only knows the rate it is listed with at home. Abroad it gets
/// the destination's rate *mapped* from that one (a book listed at 5,5 % is
/// sold at 7 % in Germany: `(550, 700)`), and the destination's standard rate
/// when nothing is mapped — never too little VAT by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DestinationRates {
    /// The destination's standard rate, basis points.
    pub standard_bp: u16,
    /// `(listed rate, destination rate)` pairs, basis points.
    pub mapped: Vec<(u16, u16)>,
}

impl DestinationRates {
    pub fn standard(standard_bp: u16) -> Self {
        Self {
            standard_bp,
            mapped: Vec::new(),
        }
    }

    /// The destination's rate for something listed at `listed_rate_bp`.
    pub fn rate_for(&self, listed_rate_bp: u16) -> u16 {
        self.mapped
            .iter()
            .find(|(listed, _)| *listed == listed_rate_bp)
            .map_or(self.standard_bp, |(_, destination)| *destination)
    }
}

/// A set of delivery countries taxed the same way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxZone {
    /// Stable identifier, recorded on orders (`"fr"`, `"fr-overseas"`).
    pub code: String,
    pub label: String,
    /// ISO 3166-1 alpha-2 delivery countries.
    pub countries: Vec<String>,
    pub treatment: TaxTreatment,
    /// The rates of [`TaxTreatment::DestinationVat`]; `None` otherwise.
    pub destination: Option<DestinationRates>,
    /// Codes of the delivery methods that serve this zone.
    pub delivery_methods: Vec<String>,
}

impl TaxZone {
    /// A zone of one country charged its own VAT at `standard_bp`; reduced
    /// rates are added with [`TaxZone::with_mapped_rate`].
    pub fn destination_vat(
        country_code: &str,
        label: &str,
        standard_bp: u16,
        delivery_methods: &[&str],
    ) -> Self {
        Self {
            code: country_code.to_ascii_lowercase(),
            label: label.to_owned(),
            countries: vec![country_code.to_ascii_uppercase()],
            treatment: TaxTreatment::DestinationVat,
            destination: Some(DestinationRates::standard(standard_bp)),
            delivery_methods: delivery_methods.iter().map(|m| (*m).to_owned()).collect(),
        }
    }

    /// Products listed at `listed_rate_bp` are sold at `destination_rate_bp`
    /// in this zone. Only means something for a destination-VAT zone.
    pub fn with_mapped_rate(mut self, listed_rate_bp: u16, destination_rate_bp: u16) -> Self {
        if let Some(rates) = &mut self.destination {
            rates.mapped.retain(|(listed, _)| *listed != listed_rate_bp);
            rates.mapped.push((listed_rate_bp, destination_rate_bp));
        }
        self
    }

    fn destination_rate_bp(&self, listed_rate_bp: u16) -> u16 {
        self.destination
            .as_ref()
            .map_or(0, |rates| rates.rate_for(listed_rate_bp))
    }

    pub fn delivers(&self, country_code: &str) -> bool {
        self.countries
            .iter()
            .any(|c| c.eq_ignore_ascii_case(country_code))
    }

    pub fn offers(&self, delivery_method: &str) -> bool {
        self.delivery_methods.iter().any(|m| m == delivery_method)
    }

    /// What is charged in this zone for something listed at
    /// `price_incl_tax`, domestic VAT of `vat_rate_bp` included.
    pub fn charged(&self, price_incl_tax: &Money, vat_rate_bp: u16) -> Money {
        match self.treatment {
            TaxTreatment::Domestic => price_incl_tax.clone(),
            TaxTreatment::Export => price_incl_tax.excl_tax(vat_rate_bp),
            TaxTreatment::DestinationVat => {
                let excl = price_incl_tax.excl_tax(vat_rate_bp);
                let rate = self.destination_rate_bp(vat_rate_bp);
                Money::new(excl.minor + excl.percent_bp(rate).minor, &excl.currency)
            }
        }
    }

    /// The rate that ends up inside what is charged.
    pub fn applied_rate_bp(&self, vat_rate_bp: u16) -> u16 {
        match self.treatment {
            TaxTreatment::Domestic => vat_rate_bp,
            TaxTreatment::Export => 0,
            TaxTreatment::DestinationVat => self.destination_rate_bp(vat_rate_bp),
        }
    }
}

/// The zones a shop delivers to. The first one is the default: what the
/// storefront prices with before it knows where the order goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaxZones {
    zones: Vec<TaxZone>,
    /// Domestic VAT rate of delivery fees, listed tax-inclusive like products.
    pub fee_vat_rate_bp: u16,
    /// Rate assumed for a product whose price cannot be found any more.
    pub fallback_vat_rate_bp: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaxZonesError {
    #[error("at least one tax zone is required")]
    Empty,
    #[error("country `{0}` is in two tax zones")]
    CountryTwice(String),
    #[error("zone `{0}` uses destination VAT without a rate")]
    MissingDestinationRate(String),
    #[error("no destination-VAT zone `{0}` to map a rate in")]
    NotADestinationZone(String),
}

/// The delivery method of the built-in EU zones.
pub const EU_DELIVERY_METHOD: &str = "colissimo-europe";

/// The other 26 member states with their standard VAT rate (basis points),
/// as in force on 1 January 2026. Rates move — Estonia, Finland, Romania and
/// Slovakia all changed theirs in 2024-2025 — so a host keeps an eye on
/// <https://ec.europa.eu/taxation_customs/tedb/> and builds its own zones
/// when this table is behind.
///
/// A zone is a whole country: the few territories of a member state that are
/// outside the EU VAT area (Canary Islands, Ceuta, Melilla, Åland, Livigno,
/// Mount Athos…) share their country's code and are not told apart.
const EU_STANDARD_RATES: &[(&str, &str, u16)] = &[
    ("DE", "Allemagne", 1_900),
    ("AT", "Autriche", 2_000),
    ("BE", "Belgique", 2_100),
    ("BG", "Bulgarie", 2_000),
    ("CY", "Chypre", 1_900),
    ("HR", "Croatie", 2_500),
    ("DK", "Danemark", 2_500),
    ("ES", "Espagne", 2_100),
    ("EE", "Estonie", 2_400),
    ("FI", "Finlande", 2_550),
    ("GR", "Grèce", 2_400),
    ("HU", "Hongrie", 2_700),
    ("IE", "Irlande", 2_300),
    ("IT", "Italie", 2_200),
    ("LV", "Lettonie", 2_100),
    ("LT", "Lituanie", 2_100),
    ("LU", "Luxembourg", 1_700),
    ("MT", "Malte", 1_800),
    ("NL", "Pays-Bas", 2_100),
    ("PL", "Pologne", 2_300),
    ("PT", "Portugal", 2_300),
    ("CZ", "République tchèque", 2_100),
    ("RO", "Roumanie", 2_100),
    ("SK", "Slovaquie", 2_300),
    ("SI", "Slovénie", 2_200),
    ("SE", "Suède", 2_500),
];

impl TaxZones {
    /// `zones[0]` is the default zone. A country may only be in one zone.
    pub fn new(zones: Vec<TaxZone>, fee_vat_rate_bp: u16) -> Result<Self, TaxZonesError> {
        if zones.is_empty() {
            return Err(TaxZonesError::Empty);
        }
        let mut seen: Vec<String> = Vec::new();
        for zone in &zones {
            if zone.treatment == TaxTreatment::DestinationVat && zone.destination.is_none() {
                return Err(TaxZonesError::MissingDestinationRate(zone.code.clone()));
            }
            for country in &zone.countries {
                let country = country.to_ascii_uppercase();
                if seen.contains(&country) {
                    return Err(TaxZonesError::CountryTwice(country));
                }
                seen.push(country);
            }
        }
        Ok(Self {
            zones,
            fee_vat_rate_bp,
            fallback_vat_rate_bp: fee_vat_rate_bp,
        })
    }

    /// Metropolitan France (with Monaco, which is French territory for VAT)
    /// taxed as listed, and the overseas departments and territories as
    /// exports. 20 % on delivery fees.
    pub fn france_with_overseas() -> Self {
        let owned = |items: &[&str]| items.iter().map(|i| (*i).to_owned()).collect();
        Self {
            zones: vec![
                TaxZone {
                    code: "fr".into(),
                    label: "France métropolitaine".into(),
                    countries: owned(&["FR", "MC"]),
                    treatment: TaxTreatment::Domestic,
                    destination: None,
                    delivery_methods: owned(&["colissimo", "store-pickup"]),
                },
                TaxZone {
                    code: "fr-overseas".into(),
                    label: "Outre-mer".into(),
                    countries: owned(&[
                        "GP", "MQ", "GF", "RE", "YT", "PM", "BL", "MF", "WF", "PF", "NC", "TF",
                    ]),
                    treatment: TaxTreatment::Export,
                    destination: None,
                    delivery_methods: owned(&["chronopost-dom"]),
                },
            ],
            fee_vat_rate_bp: 2_000,
            fallback_vat_rate_bp: 2_000,
        }
    }

    /// [`TaxZones::france_with_overseas`] plus one destination-VAT zone per
    /// other EU member state, served by [`EU_DELIVERY_METHOD`]: the shop
    /// declares those sales through the one-stop shop (OSS) and charges each
    /// customer the VAT of their country.
    ///
    /// Opt-in, because a shop under the 10 000 € a year of EU distance sales
    /// may keep charging its own VAT — it then lists those countries in its
    /// domestic zone instead. Every product gets the destination's standard
    /// rate until the host maps its reduced rates with
    /// [`TaxZones::with_mapped_rate`]; what a reduced rate covers differs
    /// from one country to the next, so none is guessed here.
    pub fn france_with_eu_oss() -> Self {
        let mut zones = Self::france_with_overseas();
        zones
            .zones
            .extend(EU_STANDARD_RATES.iter().map(|(country, label, rate)| {
                TaxZone::destination_vat(country, label, *rate, &[EU_DELIVERY_METHOD])
            }));
        zones
    }

    /// In the destination-VAT zone `zone_code`, products listed at
    /// `listed_rate_bp` are sold at `destination_rate_bp`.
    pub fn with_mapped_rate(
        mut self,
        zone_code: &str,
        listed_rate_bp: u16,
        destination_rate_bp: u16,
    ) -> Result<Self, TaxZonesError> {
        let index = self
            .zones
            .iter()
            .position(|z| z.code == zone_code && z.destination.is_some())
            .ok_or_else(|| TaxZonesError::NotADestinationZone(zone_code.to_owned()))?;
        let zone = self.zones.remove(index);
        self.zones.insert(
            index,
            zone.with_mapped_rate(listed_rate_bp, destination_rate_bp),
        );
        Ok(self)
    }

    pub fn default_zone(&self) -> &TaxZone {
        // `new` and the built-in constructor both guarantee one zone.
        &self.zones[0]
    }

    pub fn zones(&self) -> &[TaxZone] {
        &self.zones
    }

    /// The zone a delivery country belongs to; `None` when the shop does not
    /// deliver there.
    pub fn zone_of(&self, country_code: &str) -> Option<&TaxZone> {
        self.zones.iter().find(|z| z.delivers(country_code))
    }
}

impl Default for TaxZones {
    fn default() -> Self {
        Self::france_with_overseas()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_country_picks_its_zone_and_what_is_charged() {
        let zones = TaxZones::france_with_overseas();
        let listed = Money::eur(11_995);

        let metro = zones.zone_of("fr").map(|z| z.code.as_str());
        assert_eq!(metro, Some("fr"));
        assert_eq!(zones.default_zone().charged(&listed, 2_000), listed);

        let Some(overseas) = zones.zone_of("MQ") else {
            panic!("Martinique must be delivered");
        };
        assert_eq!(overseas.treatment, TaxTreatment::Export);
        // 119,95 TTC at 20 % is 99,96 HT.
        assert_eq!(overseas.charged(&listed, 2_000), Money::eur(9_996));
        assert_eq!(overseas.applied_rate_bp(2_000), 0);
        assert!(overseas.offers("chronopost-dom"));
        assert!(!overseas.offers("colissimo"));

        assert!(zones.zone_of("US").is_none());
    }

    #[test]
    fn destination_vat_swaps_the_rate() -> Result<(), TaxZonesError> {
        let zone = |code: &str, countries: &[&str], rate: Option<u16>| TaxZone {
            code: code.into(),
            label: code.into(),
            countries: countries.iter().map(|c| (*c).to_owned()).collect(),
            treatment: TaxTreatment::DestinationVat,
            destination: rate.map(DestinationRates::standard),
            delivery_methods: vec![],
        };
        let zones = TaxZones::new(vec![zone("de", &["DE"], Some(1_900))], 2_000)?;
        // 120,00 TTC at 20 % = 100,00 HT, + 19 % German VAT.
        assert_eq!(
            zones.default_zone().charged(&Money::eur(12_000), 2_000),
            Money::eur(11_900)
        );
        assert_eq!(zones.default_zone().applied_rate_bp(2_000), 1_900);

        assert_eq!(TaxZones::new(vec![], 2_000), Err(TaxZonesError::Empty));
        assert_eq!(
            TaxZones::new(vec![zone("de", &["DE"], None)], 2_000),
            Err(TaxZonesError::MissingDestinationRate("de".into()))
        );
        assert_eq!(
            TaxZones::new(
                vec![
                    zone("a", &["DE"], Some(1_900)),
                    zone("b", &["de"], Some(1_900))
                ],
                2_000
            ),
            Err(TaxZonesError::CountryTwice("DE".into()))
        );
        Ok(())
    }

    #[test]
    fn a_listed_rate_maps_to_the_destination_one() -> Result<(), TaxZonesError> {
        let zones = TaxZones::france_with_eu_oss().with_mapped_rate("de", 550, 700)?;
        let Some(germany) = zones.zone_of("de") else {
            panic!("Germany must be delivered");
        };
        // A book: 21,10 TTC at 5,5 % = 20,00 HT, + 7 % German VAT.
        assert_eq!(germany.charged(&Money::eur(2_110), 550), Money::eur(2_140));
        assert_eq!(germany.applied_rate_bp(550), 700);
        // Nothing mapped for the intermediate rate: the standard one.
        assert_eq!(germany.applied_rate_bp(1_000), 1_900);
        assert_eq!(germany.applied_rate_bp(2_000), 1_900);
        assert!(germany.offers(EU_DELIVERY_METHOD));

        // Mapping the same listed rate again replaces the first mapping.
        let zones = zones.with_mapped_rate("de", 550, 1_900)?;
        assert_eq!(
            zones.zone_of("DE").map(|z| z.applied_rate_bp(550)),
            Some(1_900)
        );

        // Only a destination-VAT zone has rates to map.
        for code in ["fr", "fr-overseas", "us"] {
            assert_eq!(
                TaxZones::france_with_eu_oss().with_mapped_rate(code, 550, 700),
                Err(TaxZonesError::NotADestinationZone(code.into()))
            );
        }
        Ok(())
    }

    #[test]
    fn the_eu_zones_are_a_valid_set_next_to_france() -> Result<(), TaxZonesError> {
        let zones = TaxZones::france_with_eu_oss();
        assert_eq!(zones.default_zone().code, "fr");
        assert_eq!(zones.zones().len(), 2 + 26);
        // No country twice, every destination zone with its rate.
        TaxZones::new(zones.zones().to_vec(), zones.fee_vat_rate_bp)?;
        for zone in &zones.zones()[2..] {
            assert_eq!(zone.treatment, TaxTreatment::DestinationVat);
            let standard = zone.applied_rate_bp(2_000);
            assert!((1_700..=2_700).contains(&standard), "{zone:?}");
            assert!(zone.treatment.regime_mention().is_some());
        }
        assert_eq!(
            zones.zone_of("fi").map(|z| z.applied_rate_bp(2_000)),
            Some(2_550)
        );
        // Domestic and export sales are what they were.
        assert_eq!(
            zones.zones()[..2],
            *TaxZones::france_with_overseas().zones()
        );
        Ok(())
    }
}
