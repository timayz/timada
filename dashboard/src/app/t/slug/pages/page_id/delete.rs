//! `POST /t/{slug}/pages/{page_id}/delete`: deletes the page for good, frees
//! its address and deletes its pictures' files.

use std::sync::Arc;

use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::{SeeOther, see_other},
        path_param, route,
    },
};

use super::super::{error_code, pages, pages_url};
use super::{PageId, page_url};
use crate::app::t::slug::require_tenant;
use crate::media::Media;

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let page_id = path_param::<PageId>(cx).to_owned();
    let slug = access.tenant.slug.clone();
    match pages(&access)?.delete_page(&access.user_id, &page_id).await {
        Ok(page) => {
            let media = app_context::<Arc<Media>>(cx);
            for image in &page.images {
                media.remove_image(&access.tenant.id, &image.id).await;
            }
            Ok(see_other(pages_url(
                cx,
                &slug,
                None,
                Some("Page deleted; the list updates in a moment."),
            )))
        }
        Err(err) if err.is_user_facing() => Ok(see_other(page_url(
            cx,
            &slug,
            &page_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
