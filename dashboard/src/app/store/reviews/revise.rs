//! `POST /store/reviews/revise`: a storefront hands in what the author of a
//! review still waiting for the merchant changed. Fields: `order_id`,
//! `product_id`, `rating`, `title` (optional), `body`, a `keep_photo` per
//! photo already on the review that stays, and the new `photo` files (at
//! most three in all). Answers: 200 `{"id": …}`; 422 `{"error": code}`,
//! `review_not_pending` once the merchant published or rejected it; and the
//! statuses of [`submit`](super::submit).

use timada_orders::STOREFRONT_ACTOR;
use timada_reviews::{REVIEW_INVALID_PHOTO, REVIEW_INVALID_RATING, ReviseReview};
use topcoat::{
    Result,
    context::Cx,
    router::{
        StatusCode,
        content::{Json, multipart::Multipart},
        route,
    },
};

use super::{Answer, answer_error, read_upload, refused, sniff_photos, status, with_photos};

#[route(POST)]
pub async fn revise(cx: &Cx, mut multipart: Multipart) -> Result<(StatusCode, Json<Answer>)> {
    let access = match crate::app::store::storefront(cx).await? {
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
    let revision = ReviseReview {
        order_id: upload.order_id,
        product_id: upload.product_id,
        rating,
        title: upload.title,
        body: upload.body,
        keep: upload.keep,
        photos: photos.iter().map(|(photo, _)| photo.clone()).collect(),
    };
    // Refused before any file is written when it can be.
    if let Err(err) = revision.validate() {
        return answer_error(err);
    }

    let store = crate::app::store::store(cx, &access);
    let reviews = timada_reviews::Command::new(store.executor().clone(), store.tenant_id());
    let revised = with_photos(cx, &access, &photos, || async {
        reviews
            .revise_review(STOREFRONT_ACTOR, revision)
            .await
            .map(|review| review.id)
    })
    .await;
    match revised {
        Ok(id) => Ok((
            StatusCode::OK,
            Json(Answer {
                id: Some(id),
                error: None,
            }),
        )),
        Err(err) => answer_error(err),
    }
}
