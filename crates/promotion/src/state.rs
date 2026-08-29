use timada_core::ServiceContext;

/// Everything the promotion routes and subscriptions need.
#[derive(Clone)]
pub struct PromotionState {
    pub ctx: ServiceContext,
}
