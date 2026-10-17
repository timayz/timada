//! The shopper keys of one tenant, as copied between nodes.

pub mod key_id;

use topcoat::router::module_param;

module_param!(pub tenant_id);
