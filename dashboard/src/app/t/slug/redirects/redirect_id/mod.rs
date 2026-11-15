//! The actions on one redirect of `/t/{slug}/redirects`.

pub mod remove;

use topcoat::router::module_param;

module_param!(pub redirect_id);
