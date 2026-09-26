// The explicit name pins the on-disk identity: renaming the crate or the enum
// must never orphan stored events.
#[evento::aggregate(name = "timada-sourcing/Supplier")]
pub enum Supplier {
    /// A supplier the shop buys from. `connector` names the adapter that
    /// talks to it — `"manual"`, `"aliexpress"`, whatever a host registers —
    /// and is a plain string on purpose: an enum nested in a stored event is
    /// frozen for ever, and the next marketplace must not need a new event
    /// shape. `currency` is what the supplier quotes its costs in.
    SupplierRegistered {
        slug: String,
        name: String,
        connector: String,
        currency: String,
    },

    SupplierRenamed {
        name: String,
    },

    /// Nothing is re-priced or bought from a suspended supplier. What it
    /// already sourced stays sourced.
    SupplierSuspended {
        reason: String,
    },

    SupplierResumed,
}

/// One of the shop's products, and where it is bought. One stream per
/// product: sourcing it from somebody else is another `ProductSourced` on the
/// same stream, which is what keeps a product to one supplier at a time.
///
/// What a supplier *says* — its cost, its shipping, how many it holds — is
/// not here. A feed polled every few hours would append events by the
/// hundred thousand and tell posterity nothing; the last word of each
/// supplier lives in `sourcing_offer`, and the consequences that matter are
/// already facts elsewhere: the price in `timada-pricing`, the level in
/// `timada-inventory`.
#[evento::aggregate(name = "timada-sourcing/SourcedProduct")]
pub enum SourcedProduct {
    ProductSourced {
        supplier_id: String,
        product_id: String,
        /// The supplier's listing.
        external_item_id: String,
        /// The supplier's own variant inside that listing; `None` when it
        /// has only one. A family's versions are separate products here, each
        /// pointing at the same item with a SKU of its own.
        external_sku: Option<String>,
    },

    SourcingStopped {
        reason: String,
    },

    /// The operator decided this price: the sync leaves it alone — it does
    /// not even ask them to look at it again.
    SourcePriceLocked {
        reason: String,
    },

    SourcePriceUnlocked,
}
