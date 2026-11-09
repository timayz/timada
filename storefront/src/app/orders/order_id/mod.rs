//! `/orders/{order_id}`: an order this browser placed, or one the shopper
//! signed in on it placed with their address, read from the store. Anyone
//! else is sent to sign in first (the link in the order's confirmation mail
//! may be opened on another device): an order id is not a password.

pub mod cancel;
pub mod documents;
pub mod review;
pub mod revise;

use timada_storefront::{
    DocumentKind, MAX_BODY_LEN, MAX_PHOTOS, MAX_TITLE_LEN, OrderStatus, ReviewState, ReviewStatus,
    documents as order_documents, format_amount_fr, menu, order, picture_url, review_of, stars,
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
use crate::app::{delivery_days, document, money, say};
use crate::shopper::holds_order;

module_param!(pub order_id);

#[topcoat::router::query_params(error = bad_request)]
pub(crate) struct OrderQuery {
    pub error: Option<String>,
    /// The product a refusal or a confirmation is about.
    pub product: Option<String>,
    pub reviewed: Option<String>,
    pub revised: Option<String>,
    /// Back from cancelling it: done, or `error` says why not.
    pub cancelled: Option<String>,
}

/// The order's page after the shopper asked to cancel it: done, or the
/// refusal.
pub fn cancelled_url(cx: &Cx, order_id: &str, error: Option<&str>) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        cancelled: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
    }
    href!(index, OrderId(order_id.to_owned()))
        .query(Link {
            cancelled: "1",
            error,
        })
        .resolve(cx)
}

/// The order's page, with a refusal or a confirmation about the review of
/// one of its products.
pub fn order_url(cx: &Cx, order_id: &str, product: &str, error: Option<&str>) -> String {
    review_url(cx, order_id, product, error, false)
}

/// The order's page after a change to a waiting review: a refusal, or a
/// confirmation of the change.
pub fn revised_url(cx: &Cx, order_id: &str, product: &str, error: Option<&str>) -> String {
    review_url(cx, order_id, product, error, true)
}

fn review_url(
    cx: &Cx,
    order_id: &str,
    product: &str,
    error: Option<&str>,
    revised: bool,
) -> String {
    #[derive(serde::Serialize)]
    struct Link<'a> {
        product: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reviewed: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        revised: Option<&'a str>,
    }
    let done = error.is_none().then_some("1");
    let link = Link {
        product,
        error,
        reviewed: done.filter(|_| !revised),
        revised: done.filter(|_| revised),
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
    /// The review as it stands, once written.
    review: Option<ReviewState>,
    /// The id and address of each of its photos, for its author to keep or
    /// drop while it waits.
    photos: Vec<(String, String)>,
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
    // none before it is paid, for a cancelled order or a shop that takes no
    // reviews.
    let mut line_reviews: Vec<LineReview> = Vec::new();
    let reviewable = matches!(order.status, OrderStatus::Paid | OrderStatus::Fulfilled);
    if reviewable && store(cx).takes_reviews() {
        for line in &order.lines {
            if line_reviews
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
            line_reviews.push(LineReview {
                product_id: line.product_id.clone(),
                name: line.name.clone(),
                review,
                photos,
            });
        }
    }
    let order_key = order_id.to_owned();
    let status = match order.status {
        OrderStatus::Authorized | OrderStatus::Paid if order.pending_cancellation.is_some() => {
            "Annulation en cours."
        }
        OrderStatus::Authorized if order.supplier.is_some() => {
            "Enregistrée : votre carte sera débitée dès que la boutique l'aura confirmée."
        }
        OrderStatus::Authorized => "Enregistrée : le paiement est en cours.",
        OrderStatus::Paid => "Payée : la boutique la prépare.",
        OrderStatus::Fulfilled => "Expédiée : elle est en route.",
        OrderStatus::Cancelled if order.is_paid() => "Annulée, et le paiement remboursé.",
        OrderStatus::Cancelled => "Annulée. Rien n'a été débité.",
    };
    let paid = order.is_paid();
    let cancellable = order.cancellable_by_shopper();
    // Back from cancelling: what came of it.
    let cancel_notice = query
        .cancelled
        .as_ref()
        .map(|_| match query.error.as_deref() {
            Some(code) => (true, say(code)),
            None => (false, "Votre commande est annulée : rien n'a été débité."),
        });
    let title = format!("Commande {}", order.reference);
    let currency = order.currency.clone();

    Ok(view! {
        document(
            title: &title,
            nav: &nav,
            <div class="wrap">
                <h1>"Merci pour votre commande !"</h1>
                <p>"Votre commande " <strong data-order-reference="">(&order.reference)</strong> " est enregistrée."</p>
                <p data-order-status=(order.status.as_str())>(status)</p>
                if let Some((refused, text)) = cancel_notice {
                    <p class="notice" role=(if refused { "alert" } else { "status" }) data-order-cancel-notice="">(text)</p>
                }
                if cancellable {
                    <details class="order-cancel" data-order-cancel="">
                        <summary>"Annuler ma commande"</summary>
                        <p>"Tant que la boutique ne l'a pas commandée à son fournisseur, votre commande s'annule ici, et rien n'est débité."</p>
                        <form method="post" action=(href!(cancel::cancel, OrderId(order_key.clone())))>
                            <button>"Confirmer l'annulation"</button>
                        </form>
                    </details>
                }
                if let Some(tracking) = &order.tracking {
                    <p>"Numéro de colis : "
                        if let Some(link) = &order.tracking_url {
                            <a data-order-tracking="" href=(link) rel="noopener" target="_blank">(tracking)</a>
                        } else {
                            <span data-order-tracking="">(tracking)</span>
                        }
                    </p>
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
                                <td>(money(line.amount_cents() as i64, &currency))</td>
                            </tr>
                        }
                    </tbody>
                </table>
                if let Some(discount) = &order.discount {
                    <p data-order-discount=(&discount.code)>"Code promo " <code>(&discount.code)</code> " : -" (money(discount.amount_cents as i64, &currency))</p>
                }
                if let Some(shipping) = &order.shipping_charge {
                    <p data-order-shipping=(&shipping.name)>"Livraison (" (&shipping.name) ") : "
                        if shipping.amount_cents == 0 { "Offerte" } else { (money(shipping.amount_cents as i64, &currency)) }
                    </p>
                }
                <p>"Total : " <strong class="price" data-order-total="">(money(order.total_cents as i64, &currency))</strong></p>
                if let Some(days) = order.delivery_window() {
                    <p data-order-delivery=(delivery_days(days))>"Livraison estimée : " (delivery_days(days)) " après la commande"</p>
                }
                <h2>"Adresse de livraison"</h2>
                <address data-order-address="">
                    (&order.shipping.name) <br>
                    (&order.shipping.line1) <br>
                    if let Some(line2) = &order.shipping.line2 { (line2) <br> }
                    (&order.shipping.postal_code) " " (&order.shipping.city) <br>
                    (&order.shipping.country)
                </address>
                if let Some(billing) = &order.billing {
                    <h2>"Adresse de facturation"</h2>
                    <address data-order-billing="">
                        (&billing.name) <br>
                        (&billing.line1) <br>
                        if let Some(line2) = &billing.line2 { (line2) <br> }
                        (&billing.postal_code) " " (&billing.city) <br>
                        (&billing.country)
                    </address>
                }
                <p>"Une confirmation est envoyée à " (&order.email) "."</p>
                if !line_reviews.is_empty() {
                    <h2 id="reviews">"Vos avis"</h2>
                    for line in &line_reviews {
                        <section class="review" id=(format!("review-{}", line.product_id)) data-review-of=(&line.product_id)>
                            <h3>(&line.name)</h3>
                            if query.product.as_deref() == Some(line.product_id.as_str()) {
                                if let Some(code) = query.error.as_deref() {
                                    <p class="notice" role="alert">(say(code))</p>
                                }
                                if query.reviewed.is_some() && query.error.is_none() {
                                    <p role="status">"Merci ! Votre avis a été transmis à la boutique."</p>
                                }
                                if query.revised.is_some() && query.error.is_none() {
                                    <p role="status">"Vos modifications ont été transmises à la boutique."</p>
                                }
                            }
                            if let Some(review) = &line.review {
                                <p data-review-status=(review.status.as_str())>
                                    <span class="rating" aria-label=(format!("{} sur 5", review.rating))>(stars(review.rating))</span> " "
                                    (match review.status {
                                        ReviewStatus::Pending => "Votre avis est en attente de publication par la boutique.",
                                        ReviewStatus::Published => "Votre avis est publié. Merci !",
                                        ReviewStatus::Rejected => "La boutique n'a pas publié votre avis.",
                                    })
                                    if let Some(reason) = review.rejection_reason.as_deref().filter(|reason| !reason.is_empty()) {
                                        " " (reason)
                                    }
                                </p>
                                if let Some(reply) = review.reply.as_deref().filter(|_| review.is_published()) {
                                    <blockquote class="review-reply" data-review-reply="">
                                        <p><strong>"Réponse de la boutique"</strong></p>
                                        <p>(reply)</p>
                                    </blockquote>
                                }
                                if review.status == ReviewStatus::Pending {
                                    <details class="review-edit" data-review-edit=(&line.product_id) open=(query.product.as_deref() == Some(line.product_id.as_str()) && query.error.is_some())>
                                        <summary>"Modifier mon avis"</summary>
                                        <form method="post" enctype="multipart/form-data" action=(href!(revise::revise, OrderId(order_key.clone())))>
                                            <input type="hidden" name="product_id" value=(&line.product_id)>
                                            <fieldset>
                                                <legend>"Votre note"</legend>
                                                for value in (1u8..=5).rev() {
                                                    <label>
                                                        <input type="radio" name="rating" value=(value) required="" checked=(value == review.rating)>
                                                        " " (value) (if value == 1 { " étoile" } else { " étoiles" })
                                                    </label> " "
                                                }
                                            </fieldset>
                                            <p>
                                                <label for=(format!("edit-title-{}", line.product_id))>"Titre (facultatif)"</label>
                                                <input id=(format!("edit-title-{}", line.product_id)) name="title" type="text" maxlength=(MAX_TITLE_LEN) value=(review.title.as_deref().unwrap_or_default())>
                                            </p>
                                            <p>
                                                <label for=(format!("edit-body-{}", line.product_id))>"Votre avis"</label>
                                                <textarea id=(format!("edit-body-{}", line.product_id)) name="body" rows="4" maxlength=(MAX_BODY_LEN) required="">(&review.body)</textarea>
                                            </p>
                                            if !line.photos.is_empty() {
                                                <fieldset>
                                                    <legend>"Vos photos"</legend>
                                                    for (id, url) in &line.photos {
                                                        <label data-review-photo-keep=(id)>
                                                            <input type="checkbox" name="keep_photo" value=(id) checked="">
                                                            <img src=(url) alt="Une photo que vous avez ajoutée" width="64" height="64">
                                                            " Conserver cette photo"
                                                        </label> " "
                                                    }
                                                </fieldset>
                                            }
                                            <p>
                                                <label for=(format!("edit-photos-{}", line.product_id))>(format!("Ajouter des photos (jusqu'à {MAX_PHOTOS} en tout)"))</label>
                                                <input id=(format!("edit-photos-{}", line.product_id)) name="photo" type="file" accept="image/jpeg,image/png,image/webp,image/gif" multiple="">
                                            </p>
                                            <button class="primary">"Enregistrer mes modifications"</button>
                                        </form>
                                    </details>
                                }
                            } else {
                                <form method="post" enctype="multipart/form-data" action=(href!(review::review, OrderId(order_key.clone())))>
                                    <input type="hidden" name="product_id" value=(&line.product_id)>
                                    <fieldset>
                                        <legend>"Votre note"</legend>
                                        for value in (1u8..=5).rev() {
                                            <label>
                                                <input type="radio" name="rating" value=(value) required="">
                                                " " (value) (if value == 1 { " étoile" } else { " étoiles" })
                                            </label> " "
                                        }
                                    </fieldset>
                                    <p>
                                        <label for=(format!("title-{}", line.product_id))>"Titre (facultatif)"</label>
                                        <input id=(format!("title-{}", line.product_id)) name="title" type="text" maxlength=(MAX_TITLE_LEN)>
                                    </p>
                                    <p>
                                        <label for=(format!("body-{}", line.product_id))>"Votre avis"</label>
                                        <textarea id=(format!("body-{}", line.product_id)) name="body" rows="4" maxlength=(MAX_BODY_LEN) required=""></textarea>
                                    </p>
                                    <p>
                                        <label for=(format!("photos-{}", line.product_id))>(format!("Photos (facultatif, jusqu'à {MAX_PHOTOS})"))</label>
                                        <input id=(format!("photos-{}", line.product_id)) name="photo" type="file" accept="image/jpeg,image/png,image/webp,image/gif" multiple="">
                                    </p>
                                    <button class="primary">"Envoyer mon avis"</button>
                                </form>
                            }
                        </section>
                    }
                }
                if paid || !documents.is_empty() {
                    <h2>"Facture"</h2>
                }
                if paid && documents.is_empty() {
                    <p data-invoice-pending="">"Votre facture est en préparation : rechargez cette page dans un instant."</p>
                }
                <ul data-documents="">
                    for doc in &documents {
                        <li>
                            <a data-document=(&doc.label) href=(href!(documents::document_id::pdf, OrderId(order_key.clone()), documents::document_id::DocumentId(doc.document_id.clone())))>
                                (if doc.kind == DocumentKind::CreditNote { "Avoir " } else { "Facture " })
                                (&doc.label)
                            </a>
                            " · " (format_amount_fr(doc.total_cents, &doc.currency)) " (PDF)"
                        </li>
                    }
                </ul>
            </div>
        )
    })
}
