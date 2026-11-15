//! One shop's feed, named by the secret its address ends with.

pub mod google;

use topcoat::router::module_param;

module_param!(pub token);
