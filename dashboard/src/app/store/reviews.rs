//! `POST /store/reviews`: a storefront hands in what a shopper wrote about a
//! product of their order, with its photos, as one multipart body. The key
//! (`Authorization: Bearer tk_…`) names the tenant and must allow
//! `store:checkout`; the storefront vouches that the shopper holds the order
//! (the order id is not guessable, and the store API lets a checkout key read
//! only what it may).
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

use std::sync::Arc;

use serde::Serialize;
use timada_catalog::{MAX_IMAGE_BYTES, new_image_id};
use timada_orders::STOREFRONT_ACTOR;
use timada_reviews::{
    REVIEW_INVALID_PHOTO, REVIEW_INVALID_RATING, ReviewError, ReviewPhoto, SubmitReview,
};
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

/// The answer to a storefront: the review's id, or why it was refused.
#[derive(Debug, Serialize)]
pub struct Answer {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'static str>,
}

fn refused(code: &'static str) -> (StatusCode, Json<Answer>) {
    (
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(Answer {
            id: None,
            error: Some(code),
        }),
    )
}

fn status(status: StatusCode) -> (StatusCode, Json<Answer>) {
    (
        status,
        Json(Answer {
            id: None,
            error: None,
        }),
    )
}

#[route(POST)]
pub async fn submit(cx: &Cx, mut multipart: Multipart) -> Result<(StatusCode, Json<Answer>)> {
    let access = match super::storefront(cx).await? {
        Ok(access) => access,
        Err(code) => return Ok(status(code)),
    };

    // The body has to be read to the end whatever happens to it.
    let mut draft = SubmitReview::default();
    let mut rating = None;
    let mut files: Vec<Vec<u8>> = Vec::new();
    while let Some(field) = multipart.next_field().await? {
        match field.name().map(str::to_owned).as_deref() {
            Some("order_id") => draft.order_id = field.text().await?,
            Some("product_id") => draft.product_id = field.text().await?,
            Some("rating") => rating = field.text().await?.trim().parse::<u8>().ok(),
            Some("title") => draft.title = Some(field.text().await?),
            Some("body") => draft.body = field.text().await?,
            Some("photo") => {
                let bytes = field.bytes().await?;
                // A file input left empty still sends a part.
                if !bytes.is_empty() {
                    files.push(bytes.to_vec());
                }
            }
            _ => {}
        }
    }
    let Some(rating) = rating else {
        return Ok(refused(REVIEW_INVALID_RATING));
    };
    draft.rating = rating;

    // What the bytes are, not what the files were called.
    let mut photos: Vec<(ReviewPhoto, Vec<u8>)> = Vec::with_capacity(files.len());
    for bytes in files {
        let sniffed: Option<SniffedImage> = Media::sniff(&bytes);
        let Some(sniffed) = sniffed.filter(|_| bytes.len() as u64 <= MAX_IMAGE_BYTES) else {
            return Ok(refused(REVIEW_INVALID_PHOTO));
        };
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
    draft.photos = photos.iter().map(|(photo, _)| photo.clone()).collect();

    let hold = super::hold(cx, &access).await?;
    let reviews = timada_reviews::Command(hold.executor().clone());
    let tenant_id = access.tenant.id.as_str();
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

    let media = app_context::<Arc<Media>>(cx).clone();
    let mut stored: Vec<String> = Vec::new();
    let mut kept = Ok(());
    for (photo, bytes) in &photos {
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
    let submitted = match kept {
        Ok(()) => reviews.submit_review(STOREFRONT_ACTOR, draft).await,
        Err(err) => Err(ReviewError::Other(err)),
    };
    match submitted {
        Ok(review) => Ok((
            StatusCode::CREATED,
            Json(Answer {
                id: Some(review.id),
                error: None,
            }),
        )),
        Err(err) => {
            for file in &stored {
                media.remove(tenant_id, file).await;
            }
            answer_error(err)
        }
    }
}

fn answer_error(err: ReviewError) -> Result<(StatusCode, Json<Answer>)> {
    match err.code() {
        Some(code) if err.is_user_facing() => Ok(refused(code)),
        _ => Err(topcoat::Error::from_anyhow(err.into())),
    }
}
