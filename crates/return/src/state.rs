use std::sync::Arc;

use timada_core::ServiceContext;
use timada_payment::PaymentProvider;

/// House rules for accepting returns.
#[derive(Debug, Clone)]
pub struct ReturnPolicy {
    /// How many days after delivery a return may still be requested.
    pub window_days: u32,
}

impl ReturnPolicy {
    pub fn window_millis(&self) -> i64 {
        i64::from(self.window_days) * 24 * 60 * 60 * 1000
    }
}

/// Everything the return routers and the return-flow subscription need. The
/// provider is here because an approved return refunds through it.
#[derive(Clone)]
pub struct ReturnState {
    pub ctx: ServiceContext,
    pub provider: Arc<dyn PaymentProvider>,
    pub policy: ReturnPolicy,
}
