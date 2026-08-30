//! Everything the order routers, the admin read model and the fulfillment saga
//! are wired with.

use std::sync::Arc;

use timada_core::ServiceContext;
use timada_customer::CustomerState;
use timada_dropship::SupplierRegistry;
use timada_payment::PaymentProvider;
use timada_tax::TaxCalculator;

/// The order context is the only one that holds every port: it owns checkout
/// and the saga, and those are what talk to tax, payment and suppliers on the
/// order's behalf.
#[derive(Clone)]
pub struct OrderState {
    pub ctx: ServiceContext,
    pub registry: SupplierRegistry,
    pub provider: Arc<dyn PaymentProvider>,
    /// Used by checkout only — tax is assessed once, at `place_order`, and
    /// then snapshotted onto the order.
    pub tax: Arc<dyn TaxCalculator>,
    /// Resolves the session cookie so checkout can attach the order to the
    /// signed-in customer and the account page can list their orders.
    pub customer: CustomerState,
}
