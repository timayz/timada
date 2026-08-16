use timada_core::Money;

/// Assesses tax for one checkout, given the destination country.
///
/// Implementations may be pure (rate tables) or call an external tax service —
/// hence async. All amounts are tax-inclusive: `gross` goes in, the
/// calculator says how much of it is tax.
#[async_trait::async_trait]
pub trait TaxCalculator: Send + Sync {
    /// Stable identifier, e.g. `"fixed-rate-vat"`.
    fn id(&self) -> &'static str;

    async fn assess(&self, req: TaxAssessmentRequest) -> Result<TaxAssessment, TaxError>;
}

/// One checkout to assess: destination country plus the gross lines.
#[derive(Debug, Clone)]
pub struct TaxAssessmentRequest {
    /// ISO 3166-1 alpha-2 country code of the shipping address.
    pub country: String,
    pub lines: Vec<TaxableLine>,
}

#[derive(Debug, Clone)]
pub struct TaxableLine {
    /// Caller-chosen correlation key (Timada passes the product id).
    pub reference: String,
    /// Tax-inclusive unit price.
    pub gross_unit_price: Money,
    pub quantity: u32,
}

/// The assessed breakdown. Per line and in total: `net + tax == gross`.
#[derive(Debug, Clone)]
pub struct TaxAssessment {
    pub lines: Vec<TaxedLine>,
    pub total_net: Money,
    pub total_tax: Money,
    pub total_gross: Money,
}

#[derive(Debug, Clone)]
pub struct TaxedLine {
    pub reference: String,
    /// Applied rate in basis points (2000 = 20 %).
    pub tax_rate_bps: u32,
    pub net: Money,
    pub tax: Money,
    pub gross: Money,
}

#[derive(Debug, thiserror::Error)]
pub enum TaxError {
    #[error("cannot assess tax for country {0:?}: {1}")]
    UnsupportedCountry(String, String),
    #[error("all lines of one assessment must share a currency")]
    MixedCurrencies,
    #[error("tax service error: {0}")]
    Api(String),
}
