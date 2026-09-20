//! Selling to a business. Its name and VAT number go on its invoices; when
//! the goods leave for another member state and the number is valid there,
//! the sale is an intra-community supply: exempt from the seller's VAT, which
//! the buyer accounts for at home ("autoliquidation"). Money-wise that is an
//! export — the pre-tax price, no VAT — which is how orders and invoices
//! record it ([`crate::TaxTreatment::Export`]); what makes it a reverse
//! charge is the [`ReverseChargeProof`] recorded next to it.

use std::sync::Arc;

use bitcode::{Decode, Encode};

use crate::{
    vat_number::{VatNumber, VatNumberValidator},
    zone::{TaxTreatment, TaxZone},
};

/// The business an order or an invoice is for. Persisted by their events.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct BusinessBuyer {
    pub company_name: String,
    /// Compact, with its country prefix: `DE123456789`.
    pub vat_number: String,
}

/// The check of the buyer's VAT number a sale without VAT rests on. Persisted
/// by the events of orders and invoices.
#[derive(Debug, Clone, PartialEq, Eq, Default, Encode, Decode)]
pub struct ReverseChargeProof {
    /// The registry's consultation number, when it gave one.
    pub consultation_ref: Option<String>,
    /// When the registry said the number was valid, Unix seconds.
    pub checked_at: u64,
}

/// A purchase made as a business: who buys, and — when the sale is exempt —
/// on what proof.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BusinessPurchase {
    pub buyer: BusinessBuyer,
    pub reverse_charge: Option<ReverseChargeProof>,
}

/// What a document says of a reverse-charged sale.
pub const REVERSE_CHARGE_MENTION: &str = "Autoliquidation de la TVA par le preneur — livraison intracommunautaire exonérée, article 262 ter I du CGI (article 138 de la directive 2006/112/CE)";

/// What a document says about the VAT regime of a sale:
/// [`TaxTreatment::regime_mention`], unless the sale was reverse-charged.
pub fn regime_mention(treatment: TaxTreatment, reverse_charged: bool) -> Option<&'static str> {
    if reverse_charged {
        Some(REVERSE_CHARGE_MENTION)
    } else {
        treatment.regime_mention()
    }
}

/// Whether a business with this VAT number, delivered in `zone`, buys without
/// the shop's VAT: the goods go to another member state (a zone taxed at
/// destination) and the number was issued by another member state than the
/// shop's — `home_countries`, the countries of the shop's own zone. Whether
/// the number is *valid* is the caller's to have checked.
pub fn qualifies_for_reverse_charge(
    zone: &TaxZone,
    vat_number: &VatNumber,
    home_countries: &[String],
) -> bool {
    zone.treatment == TaxTreatment::DestinationVat
        && !home_countries
            .iter()
            .any(|country| country.eq_ignore_ascii_case(vat_number.country_code()))
}

/// `zone` as a reverse-charged sale is priced in it: the pre-tax price and no
/// VAT, like an export. The zone keeps its code — the goods still go there.
pub fn reverse_charged(zone: &TaxZone) -> TaxZone {
    TaxZone {
        treatment: TaxTreatment::Export,
        destination: None,
        ..zone.clone()
    }
}

/// The VAT registry, in a shape subscriptions can carry as data.
#[derive(Clone)]
pub struct VatRegistry(pub Arc<dyn VatNumberValidator>);

impl VatRegistry {
    pub fn new(validator: impl VatNumberValidator + 'static) -> Self {
        Self(Arc::new(validator))
    }
}

#[cfg(test)]
mod tests {
    use timada_core::Money;

    use super::*;
    use crate::zone::TaxZones;

    #[test]
    fn only_another_member_states_business_delivered_abroad_qualifies() {
        let zones = TaxZones::france_with_eu_oss();
        let home = zones.default_zone().countries.clone();
        let zone = |country: &str| {
            zones
                .zone_of(country)
                .unwrap_or_else(|| panic!("no zone for {country}"))
        };
        let german = VatNumber::parse("DE123456789").unwrap_or_else(|e| panic!("{e}"));
        let french = VatNumber::parse("FR40303265045").unwrap_or_else(|e| panic!("{e}"));

        assert!(qualifies_for_reverse_charge(zone("DE"), &german, &home));
        // A German business delivered in Austria: still another member state.
        assert!(qualifies_for_reverse_charge(zone("AT"), &german, &home));
        // Delivered at home, or exported: nothing intra-community about it.
        assert!(!qualifies_for_reverse_charge(zone("FR"), &german, &home));
        assert!(!qualifies_for_reverse_charge(zone("MQ"), &german, &home));
        // Identified in the shop's own state: the shop's VAT applies.
        assert!(!qualifies_for_reverse_charge(zone("DE"), &french, &home));

        // Priced like an export, in the same zone.
        let exempt = reverse_charged(zone("DE"));
        assert_eq!(exempt.code, "de");
        assert_eq!(
            exempt.charged(&Money::eur(12_000), 2_000),
            Money::eur(10_000)
        );
        assert_eq!(exempt.applied_rate_bp(2_000), 0);
        assert_eq!(
            zone("DE").charged(&Money::eur(12_000), 2_000),
            Money::eur(11_900)
        );

        assert_eq!(
            regime_mention(TaxTreatment::Export, true),
            Some(REVERSE_CHARGE_MENTION)
        );
        assert_eq!(
            regime_mention(TaxTreatment::Export, false),
            TaxTreatment::Export.exemption_mention()
        );
        assert_eq!(regime_mention(TaxTreatment::Domestic, false), None);
    }
}
