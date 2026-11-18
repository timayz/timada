//! `/feeds/{token}/{market}/…`: the shop's feeds for one country and one
//! language it sells in (`be-fr`): that country's delivery prices, and
//! each product worded and linked as that language reads it. The address
//! without a market stays the whole shop's, in its main language.

pub mod google;
pub mod meta;
pub mod tiktok;

use topcoat::router::module_param;

module_param!(pub market);
