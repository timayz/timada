use timada_core::ServiceContext;

/// Everything the region routers and subscriptions need.
#[derive(Clone)]
pub struct RegionState {
    pub ctx: ServiceContext,
}
