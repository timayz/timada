//! Timada orders — and the fulfillment saga that ties the whole framework
//! together.
//!
//! Checkout turns a cart into an [`Order`]: [`place_order`] snapshots the cart
//! lines and the total onto `OrderPlaced`, then marks the cart checked out in a
//! second commit. From there nobody calls the next step directly. The
//! `order-fulfillment` subscription in [`saga`](crate::saga) listens to
//! payment, dropship and shipping events and dispatches the next command,
//! compensating when a step refuses. The `Order` aggregate doubles as the
//! saga's state: its status *is* the saga's position, so there is no second
//! state machine to keep in sync.
//!
//! Reads split the usual way. The customer's order page replays
//! [`load_order`] through the `Rw` executor, so a redirect straight out of
//! checkout shows the order that was just written. The admin list reads the
//! eventually-consistent `admin_order_list` SQL table.

mod aggregate;
mod commands;
mod migrations;
mod projections;
mod routes;
mod saga;
mod state;
mod view;

pub use aggregate::{
    Address, Order, OrderCancelled, OrderDelivered, OrderForwardedToSupplier, OrderLine, OrderPaid,
    OrderPlaced, OrderShipped,
};
pub use commands::{PlaceOrderError, allocate_discount, place_order};
pub use migrations::migrations;
pub use projections::{
    ADMIN_SUBSCRIPTION, AdminOrderRow, admin_subscription, orders_for_customer, recent_orders,
    start_subscriptions,
};
pub use routes::{admin_router, store_router};
pub use saga::{FULFILLMENT_SUBSCRIPTION, fulfillment_subscription};
pub use state::OrderState;
pub use view::{OrderStatus, OrderView, load_order};
