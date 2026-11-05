use timada_storefront::{MAX_PHOTOS, PhotoUpload, ReviewEdit, revise_review};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::multipart::Multipart,
        error::{SeeOther, not_found, see_other},
        path_param, route,
    },
};

use super::{OrderId, revised_url};
use crate::shopper::holds_order;

/// Hands in what the shopper changed in their review of one product of the
/// order while it waits for the shop, then back to the order's page.
#[route(POST)]
pub async fn revise(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let order_id = path_param::<OrderId>(cx).to_owned();
    if !holds_order(cx, &order_id).await? {
        return Err(not_found().into());
    }
    // The body has to be read to the end whatever happens to it.
    let mut product_id = String::new();
    let mut edit = ReviewEdit::default();
    let mut too_many = false;
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("product_id") => product_id = field.text().await?,
            Some("rating") => edit.rating = field.text().await?.trim().parse().unwrap_or(0),
            Some("title") => edit.title = Some(field.text().await?),
            Some("body") => edit.body = field.text().await?,
            Some("keep_photo") => edit.keep_photos.push(field.text().await?),
            Some("photo") => {
                let file_name = field.file_name().unwrap_or("photo").to_owned();
                let bytes = field.bytes().await?;
                // A file input left empty still sends a part.
                if bytes.is_empty() {
                    continue;
                }
                if edit.photos.len() == MAX_PHOTOS {
                    too_many = true;
                    continue;
                }
                edit.photos.push(PhotoUpload {
                    file_name,
                    bytes: bytes.to_vec(),
                });
            }
            _ => {}
        }
    }
    if too_many || edit.keep_photos.len() + edit.photos.len() > MAX_PHOTOS {
        return Ok(see_other(revised_url(
            cx,
            &order_id,
            &product_id,
            Some("review_too_many_photos"),
        )));
    }
    let outcome = revise_review(cx, &order_id, &product_id, edit).await?;
    let error = outcome.err().map(|refusal| refusal.code());
    Ok(see_other(revised_url(cx, &order_id, &product_id, error)))
}
