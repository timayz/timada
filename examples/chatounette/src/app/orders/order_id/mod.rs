//! `/orders/{order_id}`: an order this browser placed, or one the shopper
//! signed in on it placed with their address, read from the store. Anyone
//! else is sent to sign in first (the link in the order's confirmation mail
//! may be opened on another device): an order id is not a password.

use timada_storefront::{
    MAX_BODY_LEN, MAX_PHOTOS, MAX_TITLE_LEN, pages::order_page, review_form, revise_review_form,
    stars,
};
use topcoat::{
    Result,
    context::Cx,
    router::{module_param, page, path_param},
    view::{View, view},
};

use crate::app::document;

module_param!(pub order_id);

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let order_id = path_param::<OrderId>(cx).to_owned();
    let page = order_page(cx, &order_id).await?;
    let title = format!("Commande {}", page.order.reference);

    Ok(view! {
        document(
            title: &title,
            <h1>"Merci pour votre commande !"</h1>
            <p>"Votre commande " <strong data-order-reference="">(&page.order.reference)</strong> " est enregistrée."</p>
            <p data-order-status=(page.order.status.as_str())>(&page.status_text)</p>
            if let Some(tracking) = &page.order.tracking {
                <p>"Suivi : " (tracking)</p>
            }
            <table class="cart" data-order="">
                <tbody>
                    for item in &page.lines {
                        <tr>
                            <td>
                                (&item.line.name)
                                if let Some(label) = &item.line.variant_label { " · " (label) }
                            </td>
                            <td>"× " (item.line.quantity)</td>
                            <td>(&item.amount)</td>
                        </tr>
                    }
                </tbody>
            </table>
            if let (Some(discount), Some(amount)) = (&page.order.discount, &page.discount_amount) {
                <p data-order-discount=(&discount.code)>"Code promo " <code>(&discount.code)</code> " : " (amount)</p>
            }
            if let (Some(shipping), Some(text)) = (&page.order.shipping_charge, &page.shipping_text) {
                <p data-order-shipping=(&shipping.name)>"Livraison (" (&shipping.name) ") : " (text)</p>
            }
            <p>"Total : " <strong class="price" data-order-total="">(&page.total)</strong></p>
            <h2>"Adresse de livraison"</h2>
            <address data-order-address="">
                (&page.order.shipping.name) <br>
                (&page.order.shipping.line1) <br>
                if let Some(line2) = &page.order.shipping.line2 { (line2) <br> }
                (&page.order.shipping.postal_code) " " (&page.order.shipping.city) <br>
                (&page.order.shipping.country)
            </address>
            if let Some(billing) = &page.order.billing {
                <h2>"Adresse de facturation"</h2>
                <address data-order-billing="">
                    (&billing.name) <br>
                    (&billing.line1) <br>
                    if let Some(line2) = &billing.line2 { (line2) <br> }
                    (&billing.postal_code) " " (&billing.city) <br>
                    (&billing.country)
                </address>
            }
            <p>"Une confirmation est envoyée à " (&page.order.email) "."</p>
            if !page.reviews.is_empty() {
                <h2 id="reviews">"Vos avis"</h2>
                for line in &page.reviews {
                    <section class="review" id=(format!("review-{}", line.product_id)) data-review-of=(&line.product_id)>
                        <h3>(&line.name)</h3>
                        if let Some(flash) = &line.flash {
                            if let Some(error) = &flash.error {
                                <p class="notice" role="alert">(&error.text)</p>
                            }
                            if flash.reviewed {
                                <p role="status">"Merci ! Votre avis a été transmis à la boutique."</p>
                            }
                            if flash.revised {
                                <p role="status">"Vos modifications ont été transmises à la boutique."</p>
                            }
                        }
                        if let Some(review) = &line.review {
                            <p data-review-status=(review.status.as_str())>
                                <span class="rating" aria-label=(format!("{} sur 5", review.rating))>(stars(review.rating))</span> " "
                                (line.status_text.as_deref().unwrap_or_default())
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
                            if line.editable {
                                <details class="review-edit" data-review-edit=(&line.product_id) open=(line.edit_open)>
                                    <summary>"Modifier mon avis"</summary>
                                    revise_review_form(order_id: &order_id, product_id: &line.product_id,
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
                                    )
                                </details>
                            }
                        } else {
                            review_form(order_id: &order_id, product_id: &line.product_id,
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
                            )
                        }
                    </section>
                }
            }
            <h2>"Facture"</h2>
            if page.documents.is_empty() {
                <p data-invoice-pending="">"Votre facture est en préparation : rechargez cette page dans un instant."</p>
            }
            <ul data-documents="">
                for item in &page.documents {
                    <li>
                        <a data-document=(&item.document.label) href=(&item.url)>
                            (if item.is_credit_note { "Avoir " } else { "Facture " })
                            (&item.document.label)
                        </a>
                        " · " (&item.amount) " (PDF)"
                    </li>
                }
            </ul>
        )
    })
}
