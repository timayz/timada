//! `/t/{slug}/mail/branding`: the look all the shop's mails share, a logo,
//! an accent colour and a footer. The logo is kept like a product picture
//! (see [`crate::media`]): on a majority of nodes before it is recorded, and
//! fetched by the others as they follow the tenants' stream
//! ([`crate::media::logo_subscription`]).

pub mod clear;

use std::sync::Arc;

use bytes::Bytes;
use timada_catalog::new_image_id;
use timada_tenancy::{MailLogo, PERMISSION_MAIL_MANAGE, SetMailBranding};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::multipart::Multipart,
        error::{SeeOther, see_other},
        route,
    },
};

use super::mail_url;
use crate::app::t::slug::require_tenant;
use crate::app::tenants::{error_code, tenancy};
use crate::media::{Media, Quota, Room, STORAGE_FULL};

/// Saves the branding. A new logo replaces the old one; none uploaded keeps
/// it, unless "remove the logo" is ticked.
#[route(POST)]
pub async fn save(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_MAIL_MANAGE)
        .await?;
    let tenant = &access.tenant;
    let slug = tenant.slug.as_str();
    let refuse = |code: &str| Ok(see_other(mail_url(cx, slug, Some(code), None)));

    // The body has to be read to the end whatever happens to it.
    let mut bytes = Bytes::new();
    let mut accent = String::new();
    let mut footer = String::new();
    let mut remove_logo = false;
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("logo") => bytes = field.bytes().await?,
            Some("accent") => accent = field.text().await?,
            Some("footer") => footer = field.text().await?,
            Some("remove_logo") => remove_logo = true,
            _ => {}
        }
    }

    let mut logo = tenant
        .mail_branding
        .as_ref()
        .and_then(|branding| branding.logo.clone());
    if remove_logo {
        logo = None;
    }
    let media = app_context::<Arc<Media>>(cx).clone();
    let mut stored = None;
    if !bytes.is_empty() {
        // What the bytes are, not what the file was called.
        let rendered = match media
            .render(bytes)
            .await
            .map_err(topcoat::Error::from_anyhow)?
        {
            Ok(rendered) => rendered,
            Err(refusal) => return refuse(refusal.code()),
        };
        let room = app_context::<Arc<Quota>>(cx)
            .room_for(tenant, rendered.picture.bytes())
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        if let Room::Full { .. } = room {
            return refuse(STORAGE_FULL);
        }
        let id = new_image_id();
        // On a majority of nodes before it is recorded, as a product
        // picture is.
        let picture = media
            .keep(&tenant.id, &id, rendered)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        logo = Some(MailLogo {
            id: id.clone(),
            picture,
        });
        stored = Some(id);
    }

    let saved = tenancy(cx)
        .set_mail_branding(
            &access.user_id,
            &tenant.id,
            SetMailBranding {
                logo,
                accent: Some(accent),
                footer: Some(footer),
            },
        )
        .await;
    match saved {
        Ok(()) => Ok(see_other(mail_url(
            cx,
            slug,
            None,
            Some("Saved: all your mails carry this look."),
        ))),
        Err(err) => {
            if let Some(id) = &stored {
                media.remove_image(&tenant.id, id).await;
            }
            if err.is_user_facing() {
                refuse(error_code(&err))
            } else {
                Err(err.into())
            }
        }
    }
}
