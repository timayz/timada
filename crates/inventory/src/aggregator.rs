use crate::value_object::StockLocation;

#[evento::aggregate(name = "timada-inventory/StockItem")]
pub enum StockItem {
    /// A product started being tracked at a location.
    StockItemRegistered {
        product_id: String,
        location: StockLocation,
    },

    /// Units arrived (delivery, return to stock).
    StockReceived { quantity: u32 },

    /// Units a customer sent back went into stock again. One event per
    /// return, which is what makes restocking safe to retry.
    StockReturned { return_id: String, quantity: u32 },

    /// How many units can still be sold, as an absolute figure, set from
    /// outside the shop's own receipts: a supplier's feed, or a stock-take.
    /// What is already put aside for orders is untouched — `reserved` is a
    /// promise the shop made, not something a supplier knows about.
    ///
    /// It is the level *at that moment*, and only the next one corrects it.
    /// Sales in between lower what is left, which is the safe direction; a
    /// cancellation in between raises it, which is not — the units go back
    /// on sale although the supplier may no longer hold them. The overshoot
    /// is bounded by the cancelled quantity and lasts until the next sync,
    /// so whoever feeds these levels should ask again after a release.
    StockLevelSynced { available: u32 },

    /// Units were put aside for an order.
    StockReserved { order_id: String, quantity: u32 },

    /// An order asked for more than was available.
    StockReservationRejected {
        order_id: String,
        requested: u32,
        available: u32,
    },

    /// A reservation was given back (cancelled order, declined payment).
    StockReservationReleased { order_id: String, quantity: u32 },
}

#[evento::aggregate(name = "timada-inventory/BackInStockAlert")]
pub enum BackInStockAlert {
    /// A customer asked to be told when the product is back.
    BackInStockAlertRequested {
        product_id: String,
        customer_id: String,
        email: String,
    },

    /// The product came back and the customer was notified.
    BackInStockAlertTriggered,

    /// The customer no longer wants to be told.
    BackInStockAlertCancelled,
}
