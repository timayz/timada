use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
    session,
};

use crate::auth::identity;

#[route(POST)]
pub async fn logout(cx: &Cx) -> Result<SeeOther> {
    if let Some(hash) = session::stop(cx).await? {
        identity(cx).end_session(&hash).await?;
    }
    Ok(see_other(super::login_url(cx, None)))
}
