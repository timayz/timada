pub mod login;
pub mod register;
pub mod remove;
pub mod rename;

use serde::Serialize;
use timada_identity::IdentityError;
use topcoat::{Error, router::error::bad_request};

use crate::app::auth::error_code;
use crate::ui::describe_error;

/// What the browser script receives when a ceremony completes: where to go.
#[derive(Serialize)]
pub struct Done {
    pub location: String,
}

/// Domain errors become 400s with the same wording the pages use; anything
/// else is a 500.
pub fn into_error(err: IdentityError) -> Error {
    if err.is_user_facing() {
        bad_request(describe_error(error_code(&err))).into()
    } else {
        err.into()
    }
}
