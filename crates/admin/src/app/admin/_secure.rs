//! Everything under this group requires a signed-in admin. The group adds no
//! URL segment; the layer wraps every page below it.

pub mod customers;
pub mod orders;
pub mod products;
pub mod promotions;

use topcoat::{
    Result,
    context::Cx,
    router::{Body, Next, error::see_other, href, layer, request::uri, response::Response},
};

use crate::auth::{CurrentAdmin, current_admin};

#[layer]
async fn require_admin(cx: &Cx, body: Body, next: Next<'_>) -> Result<Response> {
    match current_admin(cx).await {
        Err(err) => Err(anyhow::anyhow!("{err:#}").into()),
        Ok(Some(admin)) => {
            let cx = cx.with(CurrentAdmin(admin.clone()));
            next.run(&cx, body).await
        }
        Ok(None) => {
            let login = href!(super::login::index)
                .query([("next", uri(cx).path())])
                .resolve(cx);
            Err(see_other(login).into())
        }
    }
}
