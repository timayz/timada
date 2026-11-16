//! `POST /t/{slug}/articles/{article_id}/delete`: deletes the article for good, frees
//! its address. Its pictures' files are deleted on every node as each
//! follows the stream ([`crate::media::subscription`]).

use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        route,
    },
};

use super::super::{articles_url, blog, error_code};
use super::{article_id, article_url};
use crate::app::t::slug::require_tenant;

#[route(POST)]
pub async fn delete(cx: &Cx) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let article_id = article_id(cx).to_owned();
    let slug = access.tenant.slug.clone();
    match blog(&access)?
        .delete_article(&access.user_id, &article_id)
        .await
    {
        Ok(_) => Ok(see_other(articles_url(
            cx,
            &slug,
            None,
            Some("Article deleted; the list updates in a moment."),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(article_url(
            cx,
            &slug,
            &article_id,
            Some(error_code(&err)),
            None,
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
