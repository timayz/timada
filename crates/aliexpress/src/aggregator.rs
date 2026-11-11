//! The persisted shapes. Every variant is frozen once merged (`events.lock`).

/// One AliExpress combination a catalog product (or one of its variants)
/// sells, and what it cost when last read. What AliExpress has of it is the
/// variant's stock in the catalog, whose streams are made for a count that
/// moves with every sale. Serialized too, as the `skus` column of the
/// `aliexpress_products` read model holds the list.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    bitcode::Encode,
    bitcode::Decode,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct SkuLink {
    pub sku_id: String,
    /// `None` for a product that does not vary.
    pub variant_id: Option<String>,
    /// In minor units of the product's currency.
    pub cost_cents: u64,
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
    /// A read found something the link did not hold, or a member waits for
    /// its outcome; `offline` once AliExpress no longer sells it. A read that
    /// changes nothing else writes nothing: the read model keeps when it was.
    Synced {
        skus: Vec<SkuLink>,
        offline: bool,
        synced_by: String,
    },
    /// The read failed, for another reason than the last one; the product
    /// kept what it had.
    SyncFailed { reason: String, failed_by: String },
    /// The product's own pricing, by cost band; `None` follows the shop's.
    PricingSet {
        pricing: Option<timada_tenancy::Pricing>,
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
    /// A read of AliExpress was asked for: by a member, who waits for its
    /// outcome, or by the dashboard on its own ([`ALIEXPRESS_ACTOR`](crate::ALIEXPRESS_ACTOR)).
    /// One node reads it ([`Share`](timada_core::Share)).
    SyncRequested { requested_by: String },
}

/// One renewal of an account's tokens: one stream per token, with the
/// deterministic id [`token_refresh_id`](crate::token_refresh_id), so a
/// single node renews it (AliExpress may retire a refresh token once used).
pub mod refresh {
    /// The renewal of one company token: one stream per token, with the
    /// deterministic id [`token_refresh_id`](crate::token_refresh_id), in
    /// the platform's own streams.
    #[evento::aggregate(name = "timada-aliexpress/TokenRefresh")]
    pub enum TokenRefresh {
        /// A node takes the renewal on, or takes it over from one that went
        /// quiet before AliExpress answered it.
        Claimed { claimed_by: String },
        /// What AliExpress handed back, recorded before the company hears
        /// of it: the only copy of the new tokens never lives in one node's
        /// memory alone.
        Renewed {
            tokens: crate::value_object::RenewedTokens,
            renewed_by: String,
        },
    }
}

/// A shop's paid order placed on AliExpress: one stream per order, with the
/// deterministic id [`forward_id`](crate::forward_id), so an order is placed
/// there once. It lives under the tenant, next to the order.
pub mod forward {
    /// One combination an order bought on AliExpress.
    #[derive(
        Debug,
        Clone,
        PartialEq,
        Eq,
        Hash,
        bitcode::Encode,
        bitcode::Decode,
        serde::Serialize,
        serde::Deserialize,
    )]
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
        /// AliExpress is about to be asked for it, written at the version
        /// the attempt took it on at: an attempt overtaken in between (asked
        /// for again, given up as stalled) asks nothing.
        Asking { asked_by: String },
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
        /// AliExpress took it a second time, as these orders, from an attempt
        /// that overlapped the one recorded: the merchant cancels them there.
        AlsoPlaced {
            ae_order_ids: Vec<String>,
            placed_by: String,
        },
        /// Nothing was placed; `reason` says why, for the merchant.
        Failed { reason: String, failed_by: String },
        /// One AliExpress order as read again, when that changed. `closed`:
        /// AliExpress closed it without shipping it (its push said so, or it
        /// finished with no parcel ever known).
        Checked {
            ae_order_id: String,
            status: String,
            logistics_status: String,
            tracking: Vec<String>,
            closed: bool,
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

/// One order AliExpress holds for a company's account, by its AliExpress
/// number: which shop's forward placed it, and what AliExpress pushed of it.
/// A push names the order and nothing else, so these streams are the
/// platform's own (no tenant), one per AliExpress order, with the
/// deterministic id [`ae_order_index_id`](crate::ae_order_index_id). Both
/// sides write to the one stream, so a push that comes before the forward
/// recorded its orders is found by that record, and the other way round.
pub mod ae_order {
    #[evento::aggregate(name = "timada-aliexpress/AeOrder")]
    pub enum AeOrder {
        /// The forward `forward_id` of the shop `tenant_id` placed it.
        Claimed {
            ae_order_id: String,
            tenant_id: String,
            forward_id: String,
        },
        /// AliExpress pushed its new status (`OrderShipped`, `OrderClosed`…),
        /// recorded before the push is answered.
        Pushed { ae_order_id: String, status: String },
    }
}

/// Which AliExpress item filled a catalog product: one stream per product,
/// with the deterministic id [`product_link_id`](crate::product_link_id),
/// written as the import records its draft. What an order's lines are bought
/// as, and whether the shopper's money waits for that purchase, is decided
/// from these streams and the item's own: the `aliexpress_products` read
/// model shows the same link, a moment later, for the pages alone.
pub mod product_link {
    #[evento::aggregate(name = "timada-aliexpress/ProductLink")]
    pub enum ProductLink {
        /// The item `link_id` filled the product; the latest stands.
        Linked {
            product_id: String,
            link_id: String,
            linked_by: String,
        },
    }
}
