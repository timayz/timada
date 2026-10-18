use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    cookie::{Cookies, cookies},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use crate::ui::theme::{RAIL_COOKIE, preference, rail_open, return_path};

#[derive(Deserialize)]
struct RailForm {
    next: Option<String>,
}

/// Folds or unfolds the rail. The rail's own button does this in the
/// browser; this is the way without scripts.
#[route(POST)]
pub async fn rail(cx: &Cx, Form(form): Form<RailForm>) -> Result<SeeOther> {
    let value = if rail_open(cx) { "closed" } else { "open" };
    cookies(cx).add(preference(RAIL_COOKIE, value));
    Ok(see_other(return_path(form.next.as_deref())))
}
