use evento::EventDetails;
use timada_shared::RequestMetadata;

pub mod migrator;
pub mod product;

pub(crate) type RequestEvent<D> = EventDetails<D, RequestMetadata>;
