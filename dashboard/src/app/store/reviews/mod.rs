//! `POST /store/reviews`: a storefront hands in what a shopper wrote about a
//! product of their order, with its photos, as one multipart body. The key
//! (`Authorization: Bearer tk_…`) names the tenant and must allow
//! `store:checkout`; the storefront vouches that the shopper holds the order
//! (the order id is not guessable, and a checkout key learns of an order only
//! by naming its id to `GET /store/orders/{order_id}`).
//!
//! Reviews go through here rather than the store API so the photos are
//! checked and kept the way product pictures are (on a majority of nodes
//! before the review is recorded), and so no key can record a review whose
//! photos are not what it says.
//!
//! Fields: `order_id`, `product_id`, `rating` (1–5), `title` (optional),
//! `body`, and up to three `photo` files. Answers: 201 `{"id": …}`; 422
//! `{"error": code}` for a refusal to show the shopper (the codes of
//! `timada_reviews::SHOPPER_CODES`); 401 for a missing, unknown or revoked
//! key, 403 for a key that may not sell, 503 for a suspended tenant.
//!
//! `POST /store/reviews/revise` ([`revise`]) changes a review that still
//! waits for the merchant, the same way.

use std::sync::Arc;

use serde::Serialize;
use timada_catalog::{MAX_IMAGE_BYTES, new_image_id};
use timada_orders::STOREFRONT_ACTOR;
use timada_reviews::{
    REVIEW_INVALID_PHOTO, REVIEW_INVALID_RATING, ReviewError, ReviewPhoto, SubmitReview,
};
use timada_tenancy::ApiKeyAccess;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        StatusCode,
        content::{Json, multipart::Multipart},
        route,
    },
};

use crate::media::{Media, SniffedImage};

pub mod revise;

/// The answer to a storefront: the review's id, or why it was refused.
#[derive(Debug, Serialize)]
pub struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

pub(super) fn refused(code: &'static str) -> (StatusCode, Json<Answer>) {
    refused_with(StatusCode::UNPROCESSABLE_ENTITY, code)
}

pub(super) fn refused_with(status: StatusCode, code: &'static str) -> (StatusCode, Json<Answer>) {
    (
        status,
        Json(Answer {
            id: None,
            error: Some(code),
        }),
    )
}

pub(super) fn status(status: StatusCode) -> (StatusCode, Json<Answer>) {
    (
        status,
        Json(Answer {
            id: None,
            error: None,
        }),
    )
}

/// What a storefront sends, read to the end whatever happens to it.
#[derive(Debug, Default)]
struct Upload {
    order_id: String,
    product_id: String,
    /// `None` when it is missing or not a number.
    rating: Option<u8>,
    title: Option<String>,
    body: String,
    /// The `keep_photo` ids of a revision.
    keep: Vec<String>,
    files: Vec<Vec<u8>>,
}

async fn read_upload(multipart: &mut Multipart) -> Result<Upload> {
    let mut upload = Upload::default();
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("order_id") => upload.order_id = field.text().await?,
            Some("product_id") => upload.product_id = field.text().await?,
            Some("rating") => upload.rating = field.text().await?.trim().parse::<u8>().ok(),
            Some("title") => upload.title = Some(field.text().await?),
            Some("body") => upload.body = field.text().await?,
            Some("keep_photo") => upload.keep.push(field.text().await?),
            Some("photo") => {
                let bytes = field.bytes().await?;
                // A file input left empty still sends a part.
                if !bytes.is_empty() {
                    upload.files.push(bytes.to_vec());
                }
            }
            _ => {}
        }
    }
    Ok(upload)
}

/// The photos the bytes are, not what the files were called; `None` when
/// one is not a picture taken, or too large.
fn sniff_photos(files: Vec<Vec<u8>>) -> Option<Vec<(ReviewPhoto, Vec<u8>)>> {
    let mut photos = Vec::with_capacity(files.len());
    for bytes in files {
        let sniffed: Option<SniffedImage> = Media::sniff(&bytes);
        let sniffed = sniffed.filter(|_| bytes.len() as u64 <= MAX_IMAGE_BYTES)?;
        let photo = ReviewPhoto {
            id: new_image_id(),
            ext: sniffed.ext.to_owned(),
            mime: sniffed.mime.to_owned(),
            size: bytes.len() as u64,
            width: sniffed.width,
            height: sniffed.height,
        };
        photos.push((photo, bytes));
    }
    Some(photos)
}

/// Stores the photos and copies them to a majority of nodes, then runs
/// `record`; on any failure, the files just stored are deleted again.
async fn with_photos<F, Fut>(
    cx: &Cx,
    access: &ApiKeyAccess,
    photos: &[(ReviewPhoto, Vec<u8>)],
    record: F,
) -> std::result::Result<String, ReviewError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = std::result::Result<String, ReviewError>>,
{
    let tenant_id = access.tenant.id.as_str();
    let media = app_context::<Arc<Media>>(cx).clone();
    let mut stored: Vec<String> = Vec::new();
    let mut kept = Ok(());
    for (photo, bytes) in photos {
        let file = photo.file();
        if let Err(err) = media.store(tenant_id, &photo.id, &photo.ext, bytes).await {
            kept = Err(err);
            break;
        }
        stored.push(file.clone());
        // On a majority of nodes before it is recorded, like the event that
        // records it.
        if let Err(err) = media.replicate(tenant_id, &file, bytes.clone()).await {
            kept = Err(err);
            break;
        }
    }
    let recorded = match kept {
        Ok(()) => record().await,
        Err(err) => Err(ReviewError::Other(err)),
    };
    if recorded.is_err() {
        for file in &stored {
            media.remove(tenant_id, file).await;
        }
    }
    recorded
}

#[route(POST)]
pub async fn submit(cx: &Cx, mut multipart: Multipart) -> Result<(StatusCode, Json<Answer>)> {
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return Ok(status(code)),
    };
    let upload = read_upload(&mut multipart).await?;
    let Some(rating) = upload.rating else {
        return Ok(refused(REVIEW_INVALID_RATING));
    };
    let Some(photos) = sniff_photos(upload.files) else {
        return Ok(refused(REVIEW_INVALID_PHOTO));
    };
    let draft = SubmitReview {
        order_id: upload.order_id,
        product_id: upload.product_id,
        rating,
        title: upload.title,
        body: upload.body,
        photos: photos.iter().map(|(photo, _)| photo.clone()).collect(),
    };

    let store = super::store(cx, &access);
    let reviews = timada_reviews::Command::new(store.executor().clone(), store.tenant_id());
    // Refused before any file is written when it can be.
    let checked = match draft.validate() {
        Ok(()) => reviews
            .reviewable_line(&draft.order_id, &draft.product_id)
            .await
            .map(|_| ()),
        Err(err) => Err(err),
    };
    if let Err(err) = checked {
        return answer_error(err);
    }

    let submitted = with_photos(cx, &access, &photos, || async {
        reviews
            .submit_review(STOREFRONT_ACTOR, draft)
            .await
            .map(|review| review.id)
    })
    .await;
    match submitted {
        Ok(id) => Ok((
            StatusCode::CREATED,
            Json(Answer {
                id: Some(id),
                error: None,
            }),
        )),
        Err(err) => answer_error(err),
    }
}

pub(super) fn answer_error(err: ReviewError) -> Result<(StatusCode, Json<Answer>)> {
    match err.code() {
        // The shop is not taking writes for the moment: not the shopper's
        // doing, so a 503 the storefront can word as "try again later".
        Some(code) if code == timada_core::WRITES_PAUSED => {
            Ok(refused_with(StatusCode::SERVICE_UNAVAILABLE, code))
        }
        Some(code) if err.is_user_facing() => Ok(refused(code)),
        _ => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shop not taking writes for the moment answers 503 with the code a
    /// storefront words as "try again later"; a refusal of the review itself
    /// stays 422, and a real failure is not an answer at all.
    #[test]
    fn a_paused_store_answers_service_unavailable() {
        let paused = ReviewError::Write(evento::WriteError::Unavailable(anyhow::anyhow!(
            "backlog at cap"
        )));
        let (status, Json(answer)) = answer_error(paused).expect("an answer");
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(answer.error, Some(timada_core::WRITES_PAUSED));

        let (status, Json(answer)) = answer_error(ReviewError::InvalidRating).expect("an answer");
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(answer.error, Some(REVIEW_INVALID_RATING));

        let broken = ReviewError::Write(evento::WriteError::Unknown(anyhow::anyhow!("disk gone")));
        assert!(answer_error(broken).is_err());
    }
}
