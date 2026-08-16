//! Everything the payment routers and subscriptions are wired with.

use std::sync::Arc;

use timada_core::ServiceContext;

use crate::provider::PaymentProvider;

#[derive(Clone)]
pub struct PaymentState {
    pub ctx: ServiceContext,
    pub provider: Arc<dyn PaymentProvider>,
}
