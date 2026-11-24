use serde::Serialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, query_params, route,
    },
};

use crate::app::t::slug::google::finish;
use crate::auth::cookies::google_flow;
use crate::auth::require_user;

#[topcoat::router::query_params(error = bad_request)]
struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

/// What Google sent back, handed on to the shop's own page.
#[derive(Serialize)]
pub struct Answer<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'a str>,
}

/// Where Google sends a member back. The flow cookie names the shop they
/// left from: its own page takes what Google answered, checks it against
/// the cookie and the member's rights in the shop, and keeps the grant.
/// Without the cookie (ten minutes passed, or another browser) there is no
/// shop to go back to.
#[route(GET)]
pub async fn callback(cx: &Cx) -> Result<SeeOther> {
    require_user(cx).await?;
    let query = query_params::<CallbackQuery>(cx)?;
    let Ok(Some(store)) = google_flow(cx).parse() else {
        return Ok(see_other("/"));
    };
    let flow = store.get();
    Ok(see_other(
        href!(finish::finish, crate::app::t::slug::slug: flow.slug)
            .query(Answer {
                code: query.code.as_deref(),
                state: query.state.as_deref(),
                error: query.error.as_deref(),
            })
            .resolve(cx),
    ))
}
