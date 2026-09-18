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
}
