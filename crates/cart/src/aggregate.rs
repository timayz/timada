use timada_core::Money;

/// What can happen to a guest cart.
///
/// `CartItemAdded` carries a full snapshot of the product rather than just a
/// reference: the storefront must be able to render and price a cart without
/// asking the catalog again, and a later re-price or delisting upstream must
/// not silently rewrite what the customer put in their cart. `supplier_id` and
/// `supplier_product_ref` ride along for the same reason — the order and the
/// supplier order downstream need to know who fulfills the line, resolved at
/// the moment it was added.
///
/// There is no `CartItemQuantityChanged`: adding the same product again is the
/// only way quantity moves up, and removing drops the whole line. That keeps
/// the aggregate to the three facts the storefront can actually produce.
#[evento::aggregate]
pub enum Cart {
    CartItemAdded {
        product_id: String,
        title: String,
        unit_price: Money,
        supplier_id: String,
        supplier_product_ref: String,
        quantity: u32,
    },
    CartItemRemoved {
        product_id: String,
    },
    /// Terminal: the cart became an order. A checked-out cart is never
    /// reopened — the customer starts a new one with a fresh id.
    CartCheckedOut,
}
