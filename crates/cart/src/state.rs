use timada_core::ServiceContext;

/// Everything the cart router needs.
///
/// No supplier registry and no read pool usage: the cart reads its own event
/// stream and the catalog's, both through the evento executor.
#[derive(Clone)]
pub struct CartState {
    pub ctx: ServiceContext,
}
