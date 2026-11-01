//! Reviews: what buyers wrote about the products of their orders.
//!
//! A shopper reviews a product from the page of an order they placed: the
//! host offers [`submit_review`] only on an order its own shopper holds
//! (typically one its cookie remembers), since the library knows nothing of
//! who is who. The review goes to the dashboard over HTTP with the photos, as
//! [`StoreConfig::api_url`](crate::StoreConfig::api_url) says, and waits for
//! the merchant; [`review_of`] reads where it stands from the store, so the
//! order page shows it at once.
//!
//! Published reviews are copied into the local database with the catalog:
//! [`reviews_of`] pages through a product's, and every [`Product`] carries
//! its count and average ([`Product::rating`]), which
//! [`ProductSort::TopRated`](crate::ProductSort::TopRated) sorts by.

use std::time::Duration;

use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use timada_core::db::{Db, FromRow, Json, Row};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

pub use timada_reviews::{
    MAX_BODY_LEN, MAX_PHOTOS, MAX_RATING, MAX_TITLE_LEN, MIN_BODY_LEN, MIN_RATING, ReviewState,
    ReviewStatus, stars,
};
use timada_reviews::{SHOPPER_CODES, review_id};

use crate::context::store;
use crate::query::Product;
use crate::store::Store;

/// A published review, as a product page shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Review {
    pub id: String,
    pub product_id: String,
    /// 1 to 5.
    pub rating: i64,
    pub title: Option<String>,
    pub body: String,
    /// `Ada L.`
    pub author_name: String,
    /// The variant bought (`Beige / Large`), if the product has variants.
    pub variant_label: Option<String>,
    pub photos: Vec<ReviewPhoto>,
    /// Unix seconds.
    pub published_at: i64,
    /// The merchant's public answer, if there is one.
    pub reply: Option<String>,
    /// Unix seconds of the answer.
    pub replied_at: Option<i64>,
}

impl FromRow for Review {
    fn from_row(row: &Row<'_>) -> anyhow::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            product_id: row.get("product_id")?,
            rating: row.get("rating")?,
            title: row.get("title")?,
            body: row.get("body")?,
            author_name: row.get("author_name")?,
            variant_label: row.get("variant_label")?,
            photos: row.get::<Json<_>>("photos_json")?.0,
            published_at: row.get("published_at")?,
            reply: row.get("reply")?,
            replied_at: row.get("replied_at")?,
        })
    }
}

impl Review {
    /// `★★★★☆`.
    pub fn stars(&self) -> String {
        stars(self.rating.clamp(0, i64::from(MAX_RATING)) as u8)
    }

    /// Points every photo at `base`, the address the store's pictures are
    /// served from, as [`Product::fill_urls`] does.
    pub fn fill_urls(&mut self, base: Option<&str>) {
        for photo in &mut self.photos {
            photo.url = match base {
                Some(base) => format!("{base}/{}", photo.file),
                None => photo.file.clone(),
            };
        }
    }
}

/// A photo of a review.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ReviewPhoto {
    pub id: String,
    /// `<id>.<ext>`, under the store's picture address.
    pub file: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Where a browser loads it from; filled in when a page reads it.
    #[serde(skip)]
    pub url: String,
}

/// How many reviews a page shows unless asked otherwise.
pub const DEFAULT_REVIEWS_PER_PAGE: u32 = 10;

/// A page of a product's published reviews, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewPage {
    pub items: Vec<Review>,
    /// 1-based.
    pub page: u32,
    pub per_page: u32,
    pub total: i64,
}

impl ReviewPage {
    pub fn pages(&self) -> u32 {
        (self.total.max(0) as u64).div_ceil(u64::from(self.per_page.max(1))) as u32
    }
}

/// One page of the published reviews of `product_id`, newest first.
pub async fn published_reviews(
    db: &Db,
    product_id: &str,
    page: u32,
    per_page: u32,
) -> anyhow::Result<ReviewPage> {
    let page = page.max(1);
    let per_page = per_page.clamp(1, crate::MAX_PER_PAGE);
    let total: i64 = db
        .fetch_scalar(
            "SELECT COUNT(*) FROM storefront_reviews WHERE product_id = ?1",
            (product_id,),
        )
        .await?;
    let items = db
        .fetch_all(
            "SELECT id, product_id, rating, title, body, author_name, variant_label, photos_json,
                published_at, reply, replied_at
             FROM storefront_reviews WHERE product_id = ?1
             ORDER BY published_at DESC, id DESC
             LIMIT ?2 OFFSET ?3",
            (
                product_id,
                i64::from(per_page),
                i64::from(page - 1) * i64::from(per_page),
            ),
        )
        .await?;
    Ok(ReviewPage {
        items,
        page,
        per_page,
        total,
    })
}

/// The latest published reviews of the whole shop, newest first.
pub async fn latest_reviews(db: &Db, limit: u32) -> anyhow::Result<Vec<Review>> {
    db.fetch_all(
        "SELECT id, product_id, rating, title, body, author_name, variant_label, photos_json,
            published_at, reply, replied_at
         FROM storefront_reviews
         ORDER BY published_at DESC, id DESC
         LIMIT ?1",
        (i64::from(limit.clamp(1, crate::MAX_PER_PAGE)),),
    )
    .await
}

/// What a shopper writes about a product of their order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewDraft {
    /// 1 to 5.
    pub rating: u8,
    /// Blank is none.
    pub title: Option<String>,
    pub body: String,
    /// At most [`MAX_PHOTOS`]; the dashboard checks what they are.
    pub photos: Vec<PhotoUpload>,
}

/// What the author of a review still waiting for the shop changes: the
/// rating, title and text replace the old ones; of the photos already on it,
/// those in `keep_photos` (their ids) stay, and `photos` come after them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReviewEdit {
    /// 1 to 5.
    pub rating: u8,
    /// Blank is none.
    pub title: Option<String>,
    pub body: String,
    pub keep_photos: Vec<String>,
    /// With the kept ones, at most [`MAX_PHOTOS`].
    pub photos: Vec<PhotoUpload>,
}

/// A file the shopper chose.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhotoUpload {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

/// Why a review was not taken, to show the shopper: one of the codes of
/// `timada_reviews::SHOPPER_CODES` (`review_invalid_body`,
/// `review_invalid_photo`, `review_already_reviewed`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReviewRefusal(pub &'static str);

impl ReviewRefusal {
    pub fn code(&self) -> &'static str {
        self.0
    }
}

/// A review taken, with its id, or refused.
pub type ReviewRefusable<T> = std::result::Result<T, ReviewRefusal>;

/// The dashboard's HTTP side: its address and the key.
pub(crate) struct Api {
    pub(crate) client: reqwest::Client,
    pub(crate) url: String,
    pub(crate) token: String,
}

impl Api {
    pub(crate) fn new(url: &str, token: &str, timeout: Duration) -> anyhow::Result<Self> {
        // rustls needs a process-wide crypto provider; installing twice is fine.
        let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
        let client = reqwest::Client::builder()
            // Photos take longer to send than a question to the store.
            .timeout(timeout.max(Duration::from_secs(30)))
            .build()
            .context("cannot prepare the HTTP client")?;
        Ok(Self {
            client,
            url: url.to_owned(),
            token: token.to_owned(),
        })
    }
}

/// The fields a review and its revision share.
fn review_form(
    order_id: &str,
    product_id: &str,
    rating: u8,
    title: Option<String>,
    body: String,
    photos: Vec<PhotoUpload>,
) -> reqwest::multipart::Form {
    let mut form = reqwest::multipart::Form::new()
        .text("order_id", order_id.to_owned())
        .text("product_id", product_id.to_owned())
        .text("rating", rating.to_string())
        .text("body", body);
    if let Some(title) = title {
        form = form.text("title", title);
    }
    for photo in photos {
        form = form.part(
            "photo",
            reqwest::multipart::Part::bytes(photo.bytes).file_name(photo.file_name),
        );
    }
    form
}

/// What the dashboard answers.
#[derive(Debug, Deserialize)]
struct Answer {
    id: Option<String>,
    error: Option<String>,
}

impl Store {
    /// Hands in a review of `product_id` from `order_id`: its id, or why it
    /// was refused. Fails when the store has no
    /// [`api_url`](crate::StoreConfig::api_url) or the dashboard cannot be
    /// reached; a key without `store:checkout` is refused there.
    pub async fn submit_review(
        &self,
        order_id: &str,
        product_id: &str,
        draft: ReviewDraft,
    ) -> anyhow::Result<ReviewRefusable<String>> {
        let form = review_form(
            order_id,
            product_id,
            draft.rating,
            draft.title,
            draft.body,
            draft.photos,
        );
        self.send_review("/store/reviews", form).await
    }

    /// Changes the review of `product_id` from `order_id` while it waits for
    /// the shop: its id, or why it was refused (`review_not_pending` once
    /// the shop published or rejected it). Fails as
    /// [`submit_review`](Self::submit_review) does.
    pub async fn revise_review(
        &self,
        order_id: &str,
        product_id: &str,
        edit: ReviewEdit,
    ) -> anyhow::Result<ReviewRefusable<String>> {
        let mut form = review_form(
            order_id,
            product_id,
            edit.rating,
            edit.title,
            edit.body,
            edit.photos,
        );
        for id in edit.keep_photos {
            form = form.text("keep_photo", id);
        }
        self.send_review("/store/reviews/revise", form).await
    }

    async fn send_review(
        &self,
        path: &str,
        form: reqwest::multipart::Form,
    ) -> anyhow::Result<ReviewRefusable<String>> {
        let api = self
            .api()
            .context("no dashboard address: set StoreConfig::api_url to take reviews")?;
        let response = api
            .client
            .post(format!("{}{path}", api.url))
            .bearer_auth(&api.token)
            .multipart(form)
            .send()
            .await
            .context("cannot reach the dashboard")?;
        let status = response.status();
        // 422: the review was refused; 503: the shop is not taking writes
        // for the moment (`writes_paused`). Both carry a code to show.
        if status.is_success() || matches!(status.as_u16(), 422 | 503) {
            let answer: Answer = response
                .json()
                .await
                .context("the dashboard's answer is not one")?;
            return match (answer.id, answer.error) {
                (Some(id), _) => Ok(Ok(id)),
                (None, Some(error)) => {
                    let code = SHOPPER_CODES
                        .iter()
                        .find(|code| **code == error)
                        .with_context(|| format!("the dashboard refused a review: {error}"))?;
                    Ok(Err(ReviewRefusal(code)))
                }
                (None, None) => anyhow::bail!("the dashboard's answer is not one"),
            };
        }
        anyhow::bail!("the dashboard did not take the review: {status}")
    }

    /// Where a browser loads a stored picture from, `file` being
    /// `<id>.<ext>`: what a page shows a waiting review's photos with.
    pub fn picture_url(&self, file: &str) -> String {
        match self.media_url() {
            Some(base) => format!("{base}/{file}"),
            None => file.to_owned(),
        }
    }

    /// The review of `product_id` from `order_id`, if one was handed in,
    /// whatever its status, read from the store.
    pub async fn review_of(
        &self,
        order_id: &str,
        product_id: &str,
    ) -> anyhow::Result<Option<ReviewState>> {
        timada_reviews::Command::unscoped(self.executor().clone())
            .load_review(&review_id(order_id, product_id))
            .await
    }

    /// The order a mailed review link opens, while the link is valid: the
    /// host then treats its visitor as holding that order. `None` for an
    /// unknown or expired token.
    pub async fn review_link(&self, token: &str) -> anyhow::Result<Option<String>> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_secs());
        timada_reviews::Command::unscoped(self.executor().clone())
            .resolve_review_link(token, now)
            .await
    }

    /// A page of the published reviews of a product, with the photos'
    /// addresses filled in.
    pub async fn reviews_of(&self, product_id: &str, page: u32) -> anyhow::Result<ReviewPage> {
        let mut reviews =
            published_reviews(self.db(), product_id, page, DEFAULT_REVIEWS_PER_PAGE).await?;
        for review in &mut reviews.items {
            review.fill_urls(self.media_url());
        }
        Ok(reviews)
    }

    /// See [`recent_reviews`].
    pub async fn recent_reviews(&self, limit: u32) -> anyhow::Result<Vec<Review>> {
        let mut reviews = latest_reviews(self.db(), limit).await?;
        for review in &mut reviews {
            review.fill_urls(self.media_url());
        }
        Ok(reviews)
    }
}

/// Hands in a review of `product_id` from `order_id`; see
/// [`Store::submit_review`].
pub async fn submit_review(
    cx: &Cx,
    order_id: &str,
    product_id: &str,
    draft: ReviewDraft,
) -> Result<ReviewRefusable<String>> {
    store(cx)
        .submit_review(order_id, product_id, draft)
        .await
        .map_err(Error::from_anyhow)
}

/// Changes a review still waiting for the shop; see
/// [`Store::revise_review`].
pub async fn revise_review(
    cx: &Cx,
    order_id: &str,
    product_id: &str,
    edit: ReviewEdit,
) -> Result<ReviewRefusable<String>> {
    store(cx)
        .revise_review(order_id, product_id, edit)
        .await
        .map_err(Error::from_anyhow)
}

/// Where a browser loads a stored picture from; see [`Store::picture_url`].
pub fn picture_url(cx: &Cx, file: &str) -> String {
    store(cx).picture_url(file)
}

/// Where the review of `product_id` from `order_id` stands, if there is one.
pub async fn review_of(cx: &Cx, order_id: &str, product_id: &str) -> Result<Option<ReviewState>> {
    store(cx)
        .review_of(order_id, product_id)
        .await
        .map_err(Error::from_anyhow)
}

/// The order a mailed review link opens; see [`Store::review_link`].
pub async fn review_link(cx: &Cx, token: &str) -> Result<Option<String>> {
    store(cx)
        .review_link(token)
        .await
        .map_err(Error::from_anyhow)
}

/// A page of the published reviews of `product`, newest first.
pub async fn reviews_of(cx: &Cx, product: &Product, page: u32) -> Result<ReviewPage> {
    store(cx)
        .reviews_of(&product.id, page)
        .await
        .map_err(Error::from_anyhow)
}

/// The latest published reviews of the whole shop, newest first: what a
/// checkout shows to reassure.
pub async fn recent_reviews(cx: &Cx, limit: u32) -> Result<Vec<Review>> {
    store(cx)
        .recent_reviews(limit)
        .await
        .map_err(Error::from_anyhow)
}
