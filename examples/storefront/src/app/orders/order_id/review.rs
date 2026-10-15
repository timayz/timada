use timada_storefront::{MAX_PHOTOS, PhotoUpload, ReviewDraft, submit_review};
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::multipart::Multipart,
        error::{SeeOther, not_found, see_other},
        path_param, route,
    },
};

use super::{OrderId, order_url};
use crate::shopper::placed;

/// Hands in the shopper's review of one product of the order, with its
/// photos, then back to the order's page.
#[route(POST)]
pub async fn review(cx: &Cx, mut multipart: Multipart) -> Result<SeeOther> {
    let order_id = path_param::<OrderId>(cx).to_owned();
    if !placed(cx, &order_id) {
        return Err(not_found().into());
    }
    // The body has to be read to the end whatever happens to it.
    let mut product_id = String::new();
    let mut rating = 0u8;
    let mut draft = ReviewDraft::default();
    let mut too_many = false;
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("product_id") => product_id = field.text().await?,
            Some("rating") => rating = field.text().await?.trim().parse().unwrap_or(0),
            Some("title") => draft.title = Some(field.text().await?),
            Some("body") => draft.body = field.text().await?,
            Some("photo") => {
                let file_name = field.file_name().unwrap_or("photo").to_owned();
                let bytes = field.bytes().await?;
                // A file input left empty still sends a part.
                if bytes.is_empty() {
                    continue;
                }
                if draft.photos.len() == MAX_PHOTOS {
                    too_many = true;
                    continue;
                }
                draft.photos.push(PhotoUpload {
                    file_name,
                    bytes: bytes.to_vec(),
                });
            }
            _ => {}
        }
    }
    if too_many {
        return Ok(see_other(order_url(
            cx,
            &order_id,
            &product_id,
            Some("review_too_many_photos"),
        )));
    }
    draft.rating = rating;
    let outcome = submit_review(cx, &order_id, &product_id, draft).await?;
    let error = outcome.err().map(|refusal| refusal.code());
    Ok(see_other(order_url(cx, &order_id, &product_id, error)))
}
