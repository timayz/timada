use timada_core::{Address, Money};

use crate::value_object::{DeliveryChoice, FulfillmentLine, OrderLine, PaymentMode, Seller};

#[evento::aggregate(name = "timada-order/Order")]
pub enum Order {
    /// "Passer commande" completed: the cart became an order.
    OrderPlaced {
        cart_id: String,
        customer_id: String,
        seller: Seller,
        lines: Vec<OrderLine>,
        delivery_address: Address,
        billing_address: Address,
        delivery: DeliveryChoice,
        payment_mode: PaymentMode,
        shipping_fee: Money,
        handling_fee: Money,
        promo_code: Option<String>,
    },

    /// The payment was captured.
    OrderPaid { payment_id: String },

    /// The parcel left with the carrier ("Expédiée le ...").
    OrderShipped {
        shipment_id: String,
        carrier: String,
        tracking_number: String,
    },

    /// The order will not be fulfilled.
    OrderCancelled { reason: String },

    /// "Renvoyer le mail de confirmation" was requested.
    OrderConfirmationResent,
}

/// Process-manager state for one order's fulfillment: reserve stock, take the
/// payment, hand over to shipping — or compensate.
#[evento::aggregate(name = "timada-order/OrderFulfillment")]
pub enum OrderFulfillment {
    FulfillmentStarted {
        order_id: String,
        lines: Vec<FulfillmentLine>,
        pickup_store_id: Option<String>,
        amount: Money,
        payment_mode: PaymentMode,
    },
    LineStockReserved {
        product_id: String,
    },
    PaymentRequested {
        payment_id: String,
    },
    PaymentCaptured,
    ShipmentRequested {
        shipment_id: String,
    },
    FulfillmentCompleted,
    FulfillmentCompensated {
        reason: String,
    },
}
