use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    cookie::{Cookies, cookies},
    router::{
        content::Form,
        error::{SeeOther, bad_request, see_other},
        route,
    },
};

use crate::ui::theme::{Scheme, THEME_COOKIE, preference, return_path};

#[derive(Deserialize)]
struct ThemeForm {
    scheme: String,
    next: Option<String>,
}

/// The colour scheme: `light`, `dark`, or `system` (which drops the cookie's
/// say and follows the operating system).
#[route(POST)]
pub async fn theme(cx: &Cx, Form(form): Form<ThemeForm>) -> Result<SeeOther> {
    let Some(scheme) = Scheme::parse(&form.scheme) else {
        return Err(bad_request("unknown scheme").into());
    };
    cookies(cx).add(preference(THEME_COOKIE, scheme.as_str()));
    Ok(see_other(return_path(form.next.as_deref())))
}
