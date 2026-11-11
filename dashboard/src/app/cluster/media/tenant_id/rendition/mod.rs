//! One rendition (`400` or `1200`) of a tenant's pictures, as copied
//! between nodes.

pub mod file;

use topcoat::router::module_param;

module_param!(pub rendition);
