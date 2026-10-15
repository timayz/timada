//! `/orders/{order_id}`: an order this browser placed, or one the shopper
//! signed in on it placed with their address, read from the store. Anyone
//! else is sent to sign in first (the link in the order's confirmation mail
//! may be opened on another device): an order id is not a password.

pub mod documents;
pub mod review;

use timada_storefront::{
    DocumentKind, MAX_BODY_LEN, MAX_PHOTOS, MAX_TITLE_LEN, OrderStatus, ReviewStatus,
    documents as order_documents, format_amount_fr, format_money, menu, order, review_of, stars,
    store,
};
use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{not_found, redirect},
        href, module_param, page, path_param, query_params,
    },
    view::{View, view},
};

use crate::app::account::sign_in::sign_in_url;
use crate::app::{document, say};
use crate::shopper::holds_order;

module_param!(pub order_id);

#[topcoat::router::query_params(error = bad_request)]
pub(crate) struct OrderQuery {
    pub error: Option<String>,
    /// The product a refusal or a confirmation is about.
    pub product: Option<String>,
    pub reviewed: Option<String>,
}

/// The order's page, with a refusal or a confirmation about the review of
/// one of its products.
pub fn order_url(cx: &Cx, order_id: &str, product: &str, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        product: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reviewed: Option<&'a str>,
    }
    let link = Link {
        product,
        error,
        reviewed: error.is_none().then_some("1"),
    };
    format!(
        "{}#review-{product}",
        href!(index, OrderId(order_id.to_owned()))
            .query(link)
            .resolve(cx)
    )
}

/// What the page shows under a product of the order: the shopper's review
/// as it stands, or the form to write one.
struct LineReview {
    product_id: String,
    name: String,
    /// Where it stands, its stars and why it was refused, once written.
    review: Option<(ReviewStatus, u8, Option<String>)>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let order_id = path_param::<OrderId>(cx);
    let Some(order) = order(cx, order_id).await? else {
        return Err(not_found().into());
    };
    if !holds_order(cx, order_id).await? {
        let here = href!(index, OrderId(order_id.to_owned())).resolve(cx);
        return Err(redirect(sign_in_url(cx, Some(&here), None)).into());
    }
    let query = query_params::<OrderQuery>(cx)?;
    let nav = menu(cx).await?;
    let documents = order_documents(cx, order_id).await?;
    // One review per product of the order, whatever variants of it it holds;
    // none for a cancelled order or a shop that takes no reviews.
    let mut line_reviews: Vec<LineReview> = Vec::new();
    if order.status != OrderStatus::Cancelled && store(cx).takes_reviews() {
        for line in &order.lines {
            if line_reviews
                .iter()
                .any(|seen| seen.product_id == line.product_id)
            {
                continue;
            }
            let review = review_of(cx, order_id, &line.product_id)
                .await?
                .map(|review| (review.status, review.rating, review.rejection_reason));
            line_reviews.push(LineReview {
                product_id: line.product_id.clone(),
                name: line.name.clone(),
                review,
            });
        }
    }
    let order_key = order_id.to_owned();
    let status = match order.status {
        OrderStatus::Paid => "Paid: the shop is preparing it.",
        OrderStatus::Fulfilled => "On its way.",
        OrderStatus::Cancelled => "Cancelled, and the payment given back.",
    };
    let title = format!("Order {}", order.reference);
    let currency = order.currency.clone();

    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <h1>"Thank you!"</h1>
            <p>"Your order " <strong data-order-reference="">(&order.reference)</strong> " is placed."</p>
            <p data-order-status=(order.status.as_str())>(status)</p>
            if let Some(tracking) = &order.tracking {
                <p>"Tracking: " (tracking)</p>
            }
            <table class="cart" data-order="">
                <tbody>
                    for line in &order.lines {
                        <tr>
                            <td>
                                (&line.name)
                                if let Some(label) = &line.variant_label { " · " (label) }
                            </td>
                            <td>"× " (line.quantity)</td>
                            <td>(format_money(line.amount_cents(), &currency))</td>
                        </tr>
                    }
                </tbody>
            </table>
            if let Some(discount) = &order.discount {
                <p data-order-discount=(&discount.code)>"Discount " <code>(&discount.code)</code> ": -" (format_money(discount.amount_cents, &currency))</p>
            }
            <p>"Total: " <strong class="price" data-order-total="">(format_money(order.total_cents, &currency))</strong></p>
            <h2>"Shipping to"</h2>
            <address data-order-address="">
                (&order.shipping.name) <br>
                (&order.shipping.line1) <br>
                if let Some(line2) = &order.shipping.line2 { (line2) <br> }
                (&order.shipping.postal_code) " " (&order.shipping.city) <br>
                (&order.shipping.country)
            </address>
            <p>"A confirmation goes to " (&order.email) "."</p>
            if !line_reviews.is_empty() {
                <h2>"Your reviews"</h2>
                for line in &line_reviews {
                    <section class="review" id=(format!("review-{}", line.product_id)) data-review-of=(&line.product_id)>
                        <h3>(&line.name)</h3>
                        if query.product.as_deref() == Some(line.product_id.as_str()) {
                            if let Some(code) = query.error.as_deref() {
                                <p class="notice" role="alert">(say(code))</p>
                            }
                            if query.reviewed.is_some() && query.error.is_none() {
                                <p role="status">"Thank you! Your review is with the shop."</p>
                            }
                        }
                        if let Some((status, rating, reason)) = &line.review {
                            <p data-review-status=(status.as_str())>
                                <span class="rating" aria-label=(format!("{rating} out of 5"))>(stars(*rating))</span> " "
                                (match status {
                                    ReviewStatus::Pending => "Your review is waiting for the shop to publish it.",
                                    ReviewStatus::Published => "Your review is published. Thank you!",
                                    ReviewStatus::Rejected => "The shop did not publish your review.",
                                })
                                if let Some(reason) = reason.as_deref().filter(|reason| !reason.is_empty()) {
                                    " " (reason)
                                }
                            </p>
                        } else {
                            <form method="post" enctype="multipart/form-data" action=(href!(review::review, OrderId(order_key.clone())))>
                                <input type="hidden" name="product_id" value=(&line.product_id)>
                                <fieldset>
                                    <legend>"Your rating"</legend>
                                    for value in (1u8..=5).rev() {
                                        <label>
                                            <input type="radio" name="rating" value=(value) required="">
                                            " " (value) (if value == 1 { " star" } else { " stars" })
                                        </label> " "
                                    }
                                </fieldset>
                                <p>
                                    <label for=(format!("title-{}", line.product_id))>"Title (optional)"</label>
                                    <input id=(format!("title-{}", line.product_id)) name="title" type="text" maxlength=(MAX_TITLE_LEN)>
                                </p>
                                <p>
                                    <label for=(format!("body-{}", line.product_id))>"Your review"</label>
                                    <textarea id=(format!("body-{}", line.product_id)) name="body" rows="4" maxlength=(MAX_BODY_LEN) required=""></textarea>
                                </p>
                                <p>
                                    <label for=(format!("photos-{}", line.product_id))>(format!("Photos (optional, up to {MAX_PHOTOS})"))</label>
                                    <input id=(format!("photos-{}", line.product_id)) name="photo" type="file" accept="image/jpeg,image/png,image/webp,image/gif" multiple="">
                                </p>
                                <button class="primary">"Send my review"</button>
                            </form>
                        }
                    </section>
                }
            }
            <h2>"Invoice"</h2>
            if documents.is_empty() {
                <p data-invoice-pending="">"Your invoice is being prepared: reload this page in a moment."</p>
            }
            <ul data-documents="">
                for doc in &documents {
                    <li>
                        <a data-document=(&doc.label) href=(href!(documents::document_id::pdf, OrderId(order_key.clone()), documents::document_id::DocumentId(doc.document_id.clone())))>
                            (if doc.kind == DocumentKind::CreditNote { "Credit note " } else { "Invoice " })
                            (&doc.label)
                        </a>
                        " · " (format_amount_fr(doc.total_cents, &doc.currency)) " (PDF)"
                    </li>
                }
            </ul>
        )
    })
}
