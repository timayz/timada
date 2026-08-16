//! Timada cart.
//!
//! A cart belongs to a browser, not to a customer: there is no identity crate
//! this pass, so the cart aggregate id is a ULID minted into the
//! [`CART_COOKIE`] cookie on the first add. That makes the cart a guest cart
//! by construction — losing the cookie loses the cart, which is the expected
//! behaviour for an anonymous storefront.
//!
//! The read model is the [`CartView`] evento projection replayed per request
//! through the `Rw` executor, not a SQL table: a cart is a handful of events
//! long, it is only ever read by the one browser that wrote it, and that
//! browser must see its own add straight away. An eventually-consistent
//! projection table would show a stale cart right after "Add to cart".
//!
//! Cart lines snapshot the product's title, price and supplier at add time.
//! The catalog may republish or re-price afterwards; what the customer put in
//! the cart is what the cart keeps showing, and it is what the order will
//! quote.

mod aggregate;
mod commands;
mod cookie;
mod routes;
mod state;
mod view;

pub use aggregate::{Cart, CartCheckedOut, CartItemAdded, CartItemRemoved};
pub use commands::{AddItemError, add_item, mark_checked_out, remove_item};
pub use cookie::{CART_COOKIE, cart_cookie_id};
pub use routes::store_router;
pub use state::CartState;
pub use view::{CartLine, CartView, load_cart};
