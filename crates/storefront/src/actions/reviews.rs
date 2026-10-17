//! Handing in a review of a product of an order, with its photos, and
//! changing one while it waits for the shop. Both only for an order the
//! browser holds ([`holds_order`]).

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::content::multipart::Multipart;
use topcoat_router::error::{SeeOther, not_found, see_other};

use crate::reviews::{
    MAX_PHOTOS, PhotoUpload, ReviewDraft, ReviewEdit, revise_review, submit_review,
};
use crate::session::holds_order;
use crate::storefront::paths;

use super::action;

/// What both forms carry: the order, the product, the rating, the title,
/// the text, the photos, and (when changing) the photos to keep.
#[derive(Default)]
struct Fields {
    order_id: String,
    product_id: String,
    rating: u8,
    title: Option<String>,
    body: String,
    keep_photos: Vec<String>,
    photos: Vec<PhotoUpload>,
    too_many: bool,
}

/// Reads the body to the end whatever happens to it.
async fn read(mut multipart: Multipart) -> Result<Fields> {
    let mut fields = Fields::default();
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("order_id") => fields.order_id = field.text().await?,
            Some("product_id") => fields.product_id = field.text().await?,
            Some("rating") => fields.rating = field.text().await?.trim().parse().unwrap_or(0),
            Some("title") => fields.title = Some(field.text().await?),
            Some("body") => fields.body = field.text().await?,
            Some("keep_photo") => fields.keep_photos.push(field.text().await?),
            Some("photo") => {
                let file_name = field.file_name().unwrap_or("photo").to_owned();
                let bytes = field.bytes().await?;
                // A file input left empty still sends a part.
                if bytes.is_empty() {
                    continue;
                }
                if fields.photos.len() == MAX_PHOTOS {
                    fields.too_many = true;
                    continue;
                }
                fields.photos.push(PhotoUpload {
                    file_name,
                    bytes: bytes.to_vec(),
                });
            }
            _ => {}
        }
    }
    Ok(fields)
}

/// Hands in the shopper's review of one product of the order, then back to
/// the order's page.
pub(crate) async fn submit(cx: &Cx, multipart: Multipart) -> Result<SeeOther> {
    let fields = read(multipart).await?;
    if !holds_order(cx, &fields.order_id).await? {
        return Err(not_found().into());
    }
    let paths = paths(cx);
    if fields.too_many {
        return Ok(see_other(paths.order_review_url(
            &fields.order_id,
            &fields.product_id,
            Some("review_too_many_photos"),
            false,
        )));
    }
    let draft = ReviewDraft {
        rating: fields.rating,
        title: fields.title,
        body: fields.body,
        photos: fields.photos,
    };
    let outcome = submit_review(cx, &fields.order_id, &fields.product_id, draft).await?;
    let error = outcome.err().map(|refusal| refusal.code());
    Ok(see_other(paths.order_review_url(
        &fields.order_id,
        &fields.product_id,
        error,
        false,
    )))
}

/// Hands in what the shopper changed in their review while it waits for the
/// shop, then back to the order's page.
pub(crate) async fn revise(cx: &Cx, multipart: Multipart) -> Result<SeeOther> {
    let fields = read(multipart).await?;
    if !holds_order(cx, &fields.order_id).await? {
        return Err(not_found().into());
    }
    let paths = paths(cx);
    if fields.too_many || fields.keep_photos.len() + fields.photos.len() > MAX_PHOTOS {
        return Ok(see_other(paths.order_review_url(
            &fields.order_id,
            &fields.product_id,
            Some("review_too_many_photos"),
            true,
        )));
    }
    let edit = ReviewEdit {
        rating: fields.rating,
        title: fields.title,
        body: fields.body,
        keep_photos: fields.keep_photos,
        photos: fields.photos,
    };
    let outcome = revise_review(cx, &fields.order_id, &fields.product_id, edit).await?;
    let error = outcome.err().map(|refusal| refusal.code());
    Ok(see_other(paths.order_review_url(
        &fields.order_id,
        &fields.product_id,
        error,
        true,
    )))
}

action!(submit_route = submit, Multipart);
action!(revise_route = revise, Multipart);
