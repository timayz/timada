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

/// A combination AliExpress sells that the shop does not: one a read found
/// after the import, offered to the merchant as a new variant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
pub struct NewSku {
    pub sku_id: String,
    /// `(option, value)` as an import names them, in AliExpress's order.
    pub selections: Vec<(String, String)>,
    /// In minor units of the product's currency.
    pub cost_cents: u64,
    pub available: u32,
    /// The address of its picture, when it has one of its own.
    pub image: Option<String>,
}

/// An AliExpress item the shop imported: one stream per item, with the
/// deterministic id [`aliexpress_product_id`](crate::aliexpress_product_id),
/// so a shop imports an item once. It lives under the tenant, next to
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
    /// What the last read found that the shop does not sell, replacing what
    /// an earlier read found. Written with the read's `Synced`.
    CombinationsFound { found: Vec<NewSku> },
    /// Combinations found earlier, now variants of the product.
    CombinationsAdded {
        skus: Vec<SkuLink>,
        added_by: String,
    },
    /// The merchant does not want this combination offered.
    CombinationIgnored { sku_id: String, ignored_by: String },
    /// Offered again after being ignored.
    CombinationRestored { sku_id: String, restored_by: String },
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

/// A shop's paid order placed on AliExpress: one stream per order, with the
/// deterministic id [`forward_id`](crate::forward_id), so an order is placed
/// there once. It lives under the tenant, next to the order.
pub mod forward {
    /// One combination an order bought on AliExpress.
    #[derive(Debug, Clone, PartialEq, Eq, Hash, bitcode::Encode, bitcode::Decode)]
    pub struct ForwardItem {
        pub ae_product_id: String,
        pub sku_id: String,
        pub quantity: u32,
        /// The delivery service asked for (`CAINIAO_STANDARD`).
        pub service_name: String,
    }

    #[evento::aggregate(name = "timada-aliexpress/Forward")]
    pub enum Forward {
        /// The order is to be placed on AliExpress (the shop places its
        /// orders as they come, or the merchant asked).
        Requested {
            order_id: String,
            requested_by: String,
        },
        /// One node took it on, so no other places it a second time.
        Started { started_by: String },
        /// AliExpress took it: its order numbers (one per seller), what was
        /// bought, whether it was asked to charge the account, and whether
        /// every line of the shop's order was bought there.
        Placed {
            ae_order_ids: Vec<String>,
            items: Vec<ForwardItem>,
            pay_requested: bool,
            complete: bool,
            placed_by: String,
        },
        /// Nothing was placed; `reason` says why, for the merchant.
        Failed { reason: String, failed_by: String },
        /// One AliExpress order as read again, when that changed.
        Checked {
            ae_order_id: String,
            status: String,
            logistics_status: String,
            tracking: Vec<String>,
        },
        /// No longer read again: `fulfilled` when the shop's order was
        /// fulfilled with AliExpress's `tracking`, else the order was settled
        /// otherwise (fulfilled by hand, cancelled, or too old).
        Done {
            tracking: Option<String>,
            fulfilled: bool,
        },
    }
}
