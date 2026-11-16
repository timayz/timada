//! One shop's feed, named by the secret its address ends with.

pub mod google;
pub mod google_reviews;
pub mod meta;
pub mod tiktok;

use topcoat::router::module_param;

module_param!(pub token);
