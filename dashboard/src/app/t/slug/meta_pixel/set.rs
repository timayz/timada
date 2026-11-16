use serde::Deserialize;
use timada_pages::TokenChange;
use timada_tenancy::PERMISSION_PAGES_MANAGE;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        route,
    },
};

use super::{MAX_TOKEN_LEN, meta_pixel_url};
use crate::app::ConversionsSealing;
use crate::app::t::slug::announcements::{error_code, pages};
use crate::app::t::slug::require_tenant;

/// The pixel as the merchant fills it in; a box left unticked posts nothing.
#[derive(Deserialize)]
pub struct MetaPixelForm {
    #[serde(default)]
    pixel_id: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    remove_token: String,
}

/// Whether `token` looks like an access token: what Meta generates is
/// letters and digits.
fn is_token(token: &str) -> bool {
    token.len() <= MAX_TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'|' | b'.'))
}

/// Sets the pixel as the form says and lands back on the page. A token
/// typed is sealed for this tenant before it is recorded.
#[route(POST)]
pub async fn set(cx: &Cx, Form(form): Form<MetaPixelForm>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_PAGES_MANAGE)
        .await?;
    let slug = access.tenant.slug.as_str();
    let refused = |code: &str| Ok(see_other(meta_pixel_url(cx, slug, Some(code), None)));
    let token = form.token.trim();
    let change = if !token.is_empty() {
        if !is_token(token) {
            return refused("meta_token_invalid");
        }
        let Some(sealer) = &context::<ConversionsSealing>(cx).0 else {
            return refused("meta_token_unavailable");
        };
        let sealed = sealer
            .seal_for(access.tenant_id(), token)
            .map_err(topcoat::Error::from_anyhow)?;
        TokenChange::Set(sealed)
    } else if !form.remove_token.is_empty() {
        TokenChange::Remove
    } else {
        TokenChange::Keep
    };
    let off = form.pixel_id.trim().is_empty();
    let set = pages(&access)?
        .set_meta_pixel(&access.user_id, Some(&form.pixel_id), change)
        .await;
    match set {
        Ok(()) if off => Ok(see_other(meta_pixel_url(
            cx,
            slug,
            None,
            Some("No pixel: the storefront takes it off in a moment."),
        ))),
        Ok(()) => Ok(see_other(meta_pixel_url(
            cx,
            slug,
            None,
            Some("Pixel saved: the storefront follows in a moment."),
        ))),
        Err(err) if err.is_user_facing() => refused(error_code(&err)),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
