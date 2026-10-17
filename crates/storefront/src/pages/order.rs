//! An order this browser placed, or one the shopper signed in on it placed
//! with their address: what it holds, its documents, and the reviews of
//! its products. Anyone else is sent to sign in first: an order id is not
//! a password. Also the page a review request's mailed link lands on.

use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::error::{not_found, redirect};

use crate::actions::query;
use crate::cart::{OrderLine, OrderState, OrderStatus, order};
use crate::context::{menu, store};
use crate::invoice::{DocumentKind, IssuedDocument, documents};
use crate::query::CategoryNode;
use crate::reviews::{ReviewState, picture_url, review_link, review_of};
use crate::session::{holds_order, remember_order};
use crate::storefront::{actions, messages, money_format, paths};

use super::{Flash, flash};

#[derive(serde::Deserialize)]
struct OrderQuery {
    error: Option<String>,
    /// The product a refusal or a confirmation is about.
    product: Option<String>,
    reviewed: Option<String>,
    revised: Option<String>,
}

/// A line of the order with its amount written the shop's way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderLineModel {
    pub line: OrderLine,
    pub amount: String,
}

/// One of the order's documents, with where its PDF is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentModel {
    pub document: IssuedDocument,
    pub url: String,
    pub amount: String,
    pub is_credit_note: bool,
}

/// What just happened to the review of one product: a refusal, or a
/// confirmation that it was handed in or changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFlash {
    pub error: Option<Flash>,
    pub reviewed: bool,
    pub revised: bool,
}

/// What the page shows under a product of the order: the shopper's review
/// as it stands, or the form to write one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineReview {
    pub product_id: String,
    pub name: String,
    /// The review as it stands, once written.
    pub review: Option<ReviewState>,
    /// What the shop says about where it stands.
    pub status_text: Option<String>,
    /// The id and address of each of its photos, for its author to keep or
    /// drop while it waits.
    pub photos: Vec<(String, String)>,
    /// What just happened to it, when the page is about it.
    pub flash: Option<ReviewFlash>,
    /// Whether to open the form to change it: a change was just refused.
    pub edit_open: bool,
    /// Whether it can still be changed.
    pub editable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderModel {
    pub nav: Vec<CategoryNode>,
    pub order: OrderState,
    pub status_text: String,
    pub currency: String,
    pub lines: Vec<OrderLineModel>,
    /// What the discount code took off, when one did.
    pub discount_amount: Option<String>,
    /// What shipping cost, or what the shop calls free; `None` when it was
    /// included.
    pub shipping_text: Option<String>,
    pub total: String,
    pub documents: Vec<DocumentModel>,
    /// One per distinct product; none for a cancelled order or a shop that
    /// takes no reviews.
    pub reviews: Vec<LineReview>,
    pub review_submit_url: String,
    pub review_revise_url: String,
    pub home_url: String,
}

/// 404 for an unknown order; a redirect to sign in for one this browser
/// does not hold.
pub async fn order_page(cx: &Cx, order_id: &str) -> Result<OrderModel> {
    let Some(order) = order(cx, order_id).await? else {
        return Err(not_found().into());
    };
    let paths = paths(cx);
    if !holds_order(cx, order_id).await? {
        let here = paths.order_url(order_id);
        return Err(redirect(paths.sign_in_url(Some(&here), None)).into());
    }
    let query: OrderQuery = query(cx)?;
    let nav = menu(cx).await?;
    let money = money_format(cx);
    let messages = messages(cx);
    let currency = order.currency.clone();
    let issued = documents(cx, order_id).await?;
    let mut reviews: Vec<LineReview> = Vec::new();
    if order.status != OrderStatus::Cancelled && store(cx).takes_reviews() {
        for line in &order.lines {
            if reviews
                .iter()
                .any(|seen| seen.product_id == line.product_id)
            {
                continue;
            }
            let review = review_of(cx, order_id, &line.product_id).await?;
            let photos = review
                .iter()
                .flat_map(|review| &review.photos)
                .map(|photo| (photo.id.clone(), picture_url(cx, &photo.file())))
                .collect();
            let about = query.product.as_deref() == Some(line.product_id.as_str());
            let flash = about.then(|| ReviewFlash {
                error: flash(cx, query.error.as_deref()),
                reviewed: query.reviewed.is_some() && query.error.is_none(),
                revised: query.revised.is_some() && query.error.is_none(),
            });
            reviews.push(LineReview {
                product_id: line.product_id.clone(),
                name: line.name.clone(),
                status_text: review
                    .as_ref()
                    .map(|review| messages.review_status(review.status).to_owned()),
                edit_open: about && query.error.is_some(),
                editable: review
                    .as_ref()
                    .is_some_and(|review| review.status == ReviewStatus::Pending),
                review,
                photos,
                flash,
            });
        }
    }
    Ok(OrderModel {
        nav,
        status_text: messages.order_status(order.status).to_owned(),
        lines: order
            .lines
            .iter()
            .map(|line| OrderLineModel {
                line: line.clone(),
                amount: money.format(line.amount_cents(), &currency),
            })
            .collect(),
        discount_amount: order
            .discount
            .as_ref()
            .map(|discount| money.signed(-(discount.amount_cents as i64), &currency)),
        shipping_text: order.shipping_charge.as_ref().map(|shipping| {
            if shipping.amount_cents == 0 {
                messages.shipping_free.to_string()
            } else {
                money.format(shipping.amount_cents, &currency)
            }
        }),
        total: money.format(order.total_cents, &currency),
        documents: issued
            .into_iter()
            .map(|document| DocumentModel {
                url: actions(cx).document_pdf(order_id, &document.document_id),
                amount: money.format(document.total_cents, &document.currency),
                is_credit_note: document.kind == DocumentKind::CreditNote,
                document,
            })
            .collect(),
        reviews,
        review_submit_url: actions(cx).review_submit(),
        review_revise_url: actions(cx).review_revise(),
        home_url: paths.home_url(),
        currency,
        order,
    })
}

use crate::reviews::ReviewStatus;

/// What the page a stale review link lands on shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewLinkModel {
    pub nav: Vec<CategoryNode>,
    pub sign_in_url: String,
}

/// The link a review request mails the buyer of a fulfilled order. A valid
/// token stands in for the browser that placed the order: the order is
/// remembered here as if this browser had placed it, and the shopper is
/// sent to its review forms. A stale or unknown token returns what the
/// page says instead.
pub async fn open_review_link(cx: &Cx, token: &str) -> Result<ReviewLinkModel> {
    let paths = paths(cx);
    if let Some(order_id) = review_link(cx, token).await? {
        remember_order(cx, &order_id)?;
        return Err(redirect(paths.order_reviews_url(&order_id)).into());
    }
    Ok(ReviewLinkModel {
        nav: menu(cx).await?,
        sign_in_url: paths.sign_in_url(None, None),
    })
}
