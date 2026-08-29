use timada_core::Currency;

/// One member country of a region: an ISO 3166-1 alpha-2 code (uppercased on
/// write) and its VAT rate in basis points (2000 = 20 %).
#[derive(Debug, Clone, PartialEq, Eq, Default, bitcode::Encode, bitcode::Decode)]
pub struct RegionCountry {
    pub code: String,
    pub tax_rate_bps: u32,
}

#[evento::aggregate]
pub enum Region {
    /// A currency zone came into being. The currency is immutable from here:
    /// carts and price tables key on it, so changing a region's currency is
    /// modeled as a new region, not an update.
    RegionCreated {
        name: String,
        currency: Currency,
        countries: Vec<RegionCountry>,
    },
    /// Name or membership changed. Carries the full new country list — a
    /// replace, not a diff, so replaying never depends on ordering subtleties.
    RegionUpdated {
        name: String,
        countries: Vec<RegionCountry>,
    },
}
