//! `/{mount}/{*rest}`: every unmatched admin URL renders the branded 404
//! through the admin layout instead of the router's bare "not found".

use topcoat::{
    Result,
    router::{error::not_found, page, path_param},
    view::View,
};

path_param!(pub *rest);

#[page(*)]
pub async fn not_found_page() -> Result<impl View> {
    Err::<(), _>(not_found().into())
}
