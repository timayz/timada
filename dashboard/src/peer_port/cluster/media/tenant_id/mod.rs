//! The pictures of one tenant, as copied between nodes.

pub mod rendition;

use topcoat::router::module_param;

module_param!(pub tenant_id);
