//! Everything the order routers, the admin read model and the fulfillment saga
//! are wired with.

use std::sync::Arc;

use timada_core::ServiceContext;
use timada_dropship::SupplierRegistry;
use timada_payment::PaymentProvider;

/// The order context is the only one that holds all three ports: it owns the
/// saga, and the saga is what talks to payment and to suppliers on the order's
/// behalf.
#[derive(Clone)]
pub struct OrderState {
    pub ctx: ServiceContext,
    pub registry: SupplierRegistry,
    pub provider: Arc<dyn PaymentProvider>,
}
