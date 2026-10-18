//! The persisted shapes. Every variant is frozen once merged (`events.lock`).

/// One AliExpress combination a catalog product (or one of its variants)
/// sells, with what it cost and how many AliExpress had when last read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct SkuLink {
    pub sku_id: String,
    /// `None` for a product that does not vary.
    pub variant_id: Option<String>,
    /// In minor units of the product's currency.
    pub cost_cents: u64,
    pub available: u32,
}

/// An AliExpress item the shop imported: one stream per item, with the
/// deterministic id [`aliexpress_product_id`](crate::aliexpress_product_id),
/// so a shop imports an item once. It lives in the tenant's store, next to
/// the catalog product it filled.
#[evento::aggregate(name = "timada-aliexpress/Product")]
pub enum AliexpressProduct {
    /// The merchant asked for it; the dashboard fetches and imports it.
    ImportRequested {
        ae_product_id: String,
        requested_by: String,
    },
    /// One node took the import on, so no other fills a second product.
    ImportStarted { started_by: String },
    /// The draft it filled, and the combinations each variant is.
    Imported {
        product_id: String,
        skus: Vec<SkuLink>,
        imported_by: String,
    },
    /// Nothing was imported; `reason` says why, for the merchant.
    ImportFailed { reason: String, failed_by: String },
    /// Read again from AliExpress; `offline` once it no longer sells it.
    Synced {
        skus: Vec<SkuLink>,
        offline: bool,
        synced_by: String,
    },
    /// The read failed; the product kept what it had.
    SyncFailed { reason: String, failed_by: String },
    /// The product's own price rule; `None` follows the shop's.
    PricingSet {
        pricing: Option<timada_tenancy::PricingRule>,
        set_by: String,
    },
    /// No longer kept in step; the catalog product stays as it is.
    Unlinked { unlinked_by: String },
}

/// One renewal of an account's tokens: one stream per token, with the
/// deterministic id [`token_refresh_id`](crate::token_refresh_id), so a
/// single node renews it (AliExpress may retire a refresh token once used).
pub mod refresh {
    #[evento::aggregate(name = "timada-aliexpress/TokenRefresh")]
    pub enum TokenRefresh {
        Claimed { claimed_by: String },
    }
}
