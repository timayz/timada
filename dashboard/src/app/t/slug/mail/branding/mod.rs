//! `/t/{slug}/mail/branding`: the look all the shop's mails share, a logo,
//! an accent colour and a footer. The logo is kept like a product picture
//! (see [`crate::media`]): on a majority of nodes before it is recorded, and
//! fetched by the others as they follow the tenants' stream
//! ([`crate::media::logo_subscription`]).

pub mod clear;

use std::sync::Arc;

use timada_catalog::{MAX_IMAGE_BYTES, new_image_id};
use timada_tenancy::{PERMISSION_MAIL_MANAGE, SetMailBranding};
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
use crate::media::Media;

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
    let mut bytes: Vec<u8> = Vec::new();
    let mut accent = String::new();
    let mut footer = String::new();
    let mut remove_logo = false;
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("logo") => bytes = field.bytes().await?.to_vec(),
            Some("accent") => accent = field.text().await?,
            Some("footer") => footer = field.text().await?,
            Some("remove_logo") => remove_logo = true,
            _ => {}
        }
    }

    let current = tenant.mail_branding.as_ref();
    let (mut logo_file, mut logo_size) = current
        .and_then(|branding| {
            branding
                .logo_file
                .clone()
                .map(|file| (Some(file), branding.logo_size))
        })
        .unwrap_or((None, 0));
    if remove_logo {
        (logo_file, logo_size) = (None, 0);
    }
    let media = app_context::<Arc<Media>>(cx).clone();
    let mut stored = None;
    if !bytes.is_empty() {
        if bytes.len() as u64 > MAX_IMAGE_BYTES {
            return refuse("image_too_large");
        }
        // What the bytes are, not what the file was called.
        let Some(sniffed) = Media::sniff(&bytes) else {
            return refuse("unsupported_image");
        };
        let id = new_image_id();
        let file = format!("{id}.{}", sniffed.ext);
        media
            .store(&tenant.id, &id, sniffed.ext, &bytes)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
        // On a majority of nodes before it is recorded, as a product
        // picture is.
        if let Err(err) = media.replicate(&tenant.id, &file, bytes.clone()).await {
            media.remove(&tenant.id, &file).await;
            return Err(topcoat::Error::from_anyhow(err));
        }
        (logo_file, logo_size) = (Some(file.clone()), bytes.len() as u64);
        stored = Some(file);
    }

    let saved = tenancy(cx)
        .set_mail_branding(
            &access.user_id,
            &tenant.id,
            SetMailBranding {
                logo_file,
                logo_size,
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
            if let Some(file) = &stored {
                media.remove(&tenant.id, file).await;
            }
            if err.is_user_facing() {
                refuse(error_code(&err))
            } else {
                Err(err.into())
            }
        }
    }
}
