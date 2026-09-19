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
    /// The VAT rate (basis points) of [`TaxTreatment::DestinationVat`].
    pub destination_rate_bp: Option<u16>,
    /// Codes of the delivery methods that serve this zone.
    pub delivery_methods: Vec<String>,
}

impl TaxZone {
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
                let rate = self.destination_rate_bp.unwrap_or(0);
                Money::new(excl.minor + excl.percent_bp(rate).minor, &excl.currency)
            }
        }
    }

    /// The rate that ends up inside what is charged.
    pub fn applied_rate_bp(&self, vat_rate_bp: u16) -> u16 {
        match self.treatment {
            TaxTreatment::Domestic => vat_rate_bp,
            TaxTreatment::Export => 0,
            TaxTreatment::DestinationVat => self.destination_rate_bp.unwrap_or(0),
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
}

impl TaxZones {
    /// `zones[0]` is the default zone. A country may only be in one zone.
    pub fn new(zones: Vec<TaxZone>, fee_vat_rate_bp: u16) -> Result<Self, TaxZonesError> {
        if zones.is_empty() {
            return Err(TaxZonesError::Empty);
        }
        let mut seen: Vec<String> = Vec::new();
        for zone in &zones {
            if zone.treatment == TaxTreatment::DestinationVat && zone.destination_rate_bp.is_none()
            {
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
                    destination_rate_bp: None,
                    delivery_methods: owned(&["colissimo", "store-pickup"]),
                },
                TaxZone {
                    code: "fr-overseas".into(),
                    label: "Outre-mer".into(),
                    countries: owned(&[
                        "GP", "MQ", "GF", "RE", "YT", "PM", "BL", "MF", "WF", "PF", "NC", "TF",
                    ]),
                    treatment: TaxTreatment::Export,
                    destination_rate_bp: None,
                    delivery_methods: owned(&["chronopost-dom"]),
                },
            ],
            fee_vat_rate_bp: 2_000,
            fallback_vat_rate_bp: 2_000,
        }
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
        let zone = |code: &str, countries: &[&str], rate| TaxZone {
            code: code.into(),
            label: code.into(),
            countries: countries.iter().map(|c| (*c).to_owned()).collect(),
            treatment: TaxTreatment::DestinationVat,
            destination_rate_bp: rate,
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
}
