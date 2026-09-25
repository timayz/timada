//! `/account/orders/{order_id}/invoice`: the shopper's invoice as a print-ready
//! page — its own document, no shop header on an invoice — and
//! `/account/orders/{order_id}/invoice.pdf`, the same document as a file the
//! server renders ("Télécharger la facture"). Each credit note of the order
//! is a file too: `/account/orders/{order_id}/credit-notes/{credit_note_id}`.

use timada_invoice::{
    ArchivePolicy, InvoiceDocument, archive_credit_note, archive_invoice,
    credit_note_pdf_file_name, invoice_id, invoice_pdf_file_name, load_credit_note_document,
    load_invoice_document,
};
use timada_order::load_order_details;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        Body,
        error::RouterErrorExt,
        href, page, path_param, path_param as param,
        response::{IntoResponse, Response},
        route,
    },
    view::{Unescaped, View, view},
};

use super::{
    account,
    checkout::OrderId,
    format::{address_lines, date, money, vat_rate},
};
use crate::{Store, db::invoice_issuer, guest::require_shopper};

/// The sheet itself; the palette comes from the shop's own tokens.
const STYLES: &str = include_str!("invoice.css");

/// The invoice of the signed-in shopper's order, once it is issued. Someone
/// else's order, or an invoice not issued yet, is a 404.
async fn own_invoice(cx: &Cx) -> Result<(String, InvoiceDocument)> {
    let shopper = require_shopper(cx).await?;
    let order_id = param::<OrderId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    load_order_details(&store.executor, &order_id)
        .await?
        .filter(|order| order.customer_id == shopper.customer_id)
        .ok_or_not_found()?;
    let document = load_invoice_document(
        &store.executor,
        &store.db,
        &invoice_issuer(),
        &invoice_id(&order_id),
    )
    .await?
    .ok_or_not_found()?;
    Ok((order_id, document))
}

/// A PDF handed to the browser as a file to keep.
pub struct PdfDownload {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

impl IntoResponse for PdfDownload {
    fn into_response(self, _cx: &Cx) -> Result<Response> {
        Ok(Response::builder()
            .header("Content-Type", "application/pdf")
            .header(
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", self.file_name),
            )
            // Personal data: neither shared caches nor the back button keep it.
            .header("Cache-Control", "private, no-store")
            .body(Body::from(self.bytes))?)
    }
}

#[route(GET "/account/orders/{order_id}/invoice.pdf")]
pub async fn pdf(cx: &Cx) -> Result<PdfDownload> {
    let (order_id, document) = own_invoice(cx).await?;
    let store = app_context::<Store>(cx);
    // The archived file — filed now if the archive has not caught up yet:
    // what is downloaded today is what will be downloaded in ten years.
    let (_, bytes) = archive_invoice(
        &store.executor,
        &store.db,
        store.archive.0.as_ref(),
        &invoice_issuer(),
        &invoice_id(&order_id),
        &ArchivePolicy::default(),
    )
    .await
    .map_err(anyhow::Error::from)?
    .ok_or_not_found()?;
    Ok(PdfDownload {
        file_name: invoice_pdf_file_name(&document),
        bytes,
    })
}

path_param!(pub credit_note_id: String, error = not_found);

/// A credit note ("avoir") of the signed-in shopper's order, as the archived
/// file. Someone else's order, or a credit note of another order, is a 404.
#[route(GET "/account/orders/{order_id}/credit-notes/{credit_note_id}")]
pub async fn credit_note_pdf(cx: &Cx) -> Result<PdfDownload> {
    let shopper = require_shopper(cx).await?;
    let order_id = param::<OrderId>(cx)?.clone();
    let note_id = param::<CreditNoteId>(cx)?.clone();
    let store = app_context::<Store>(cx);
    load_order_details(&store.executor, &order_id)
        .await?
        .filter(|order| order.customer_id == shopper.customer_id)
        .ok_or_not_found()?;
    let document = load_credit_note_document(&store.executor, &invoice_issuer(), &note_id)
        .await?
        .filter(|document| document.order_id == order_id)
        .ok_or_not_found()?;
    let (_, bytes) = archive_credit_note(
        &store.executor,
        &store.db,
        store.archive.0.as_ref(),
        &invoice_issuer(),
        &note_id,
        &ArchivePolicy::default(),
    )
    .await
    .map_err(anyhow::Error::from)?
    .ok_or_not_found()?;
    Ok(PdfDownload {
        file_name: credit_note_pdf_file_name(&document),
        bytes,
    })
}

#[page("/account/orders/{order_id}/invoice")]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let (order_id, document) = own_invoice(cx).await?;

    let title = format!("Facture {}", document.number);
    let download = href!(pdf, OrderId(order_id.clone())).resolve(cx);
    let back = href!(account::order_detail, OrderId(order_id)).resolve(cx);
    let price_heading = if document.amounts_include_vat {
        "Prix unitaire TTC"
    } else {
        "Prix unitaire HT"
    };
    let total_heading = if document.amounts_include_vat {
        "Total TTC"
    } else {
        "Total HT"
    };
    let lines: Vec<(String, String, String, String)> = document
        .lines
        .iter()
        .map(|l| {
            (
                l.label.clone(),
                l.quantity.to_string(),
                money(&l.unit_price),
                money(&l.total),
            )
        })
        .collect();
    let vat_lines: Vec<(String, String, String, String)> = document
        .vat_lines
        .iter()
        .map(|l| {
            (
                vat_rate(l.rate_bp),
                money(&l.base),
                money(&l.vat),
                money(&l.total),
            )
        })
        .collect();
    let credit_notes: Vec<(String, String, String, String)> = document
        .credit_notes
        .iter()
        .map(|n| {
            (
                n.number.clone(),
                date(n.issued_at),
                n.reason.clone(),
                money(&n.amount),
            )
        })
        .collect();
    let buyer: Vec<String> = document
        .company_lines()
        .into_iter()
        .chain(address_lines(&document.buyer))
        .collect();
    let discount = document
        .discount
        .as_ref()
        .map(|(label, amount)| (label.clone(), money(amount)));
    let totals_excl_vat = document
        .total_excl_vat()
        .zip(document.vat_total())
        .filter(|_| document.amounts_include_vat)
        .map(|(base, vat)| (money(&base), money(&vat)));

    Ok(view! {
        <!DOCTYPE html>
        <html lang="fr">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>(title.clone()) " · " (document.issuer.name.clone())</title>
                <style>(Unescaped::new_unchecked(super::TOKENS))(Unescaped::new_unchecked(STYLES))</style>
            </head>
            <body>
                <main>
                    <div class="actions">
                        <button type="button" onclick="window.print()">"Imprimer ou enregistrer en PDF"</button>
                        <span class="muted">"ou Ctrl+P / ⌘P"</span>
                        <a href=(download)>"Télécharger le PDF"</a>
                        <a href=(back)>"Retour à la commande"</a>
                    </div>
                    <header class="doc">
                        <div>
                            <h1>(title.clone())</h1>
                            <p>"Date : " (date(document.issued_at)) <br> "Commande : " (document.order_label.clone())</p>
                        </div>
                        <address>
                            <strong>(document.issuer.name.clone())</strong> <br>
                            for line in &document.issuer.address_lines { (line.clone()) <br> }
                            (document.issuer.registration.clone()) <br>
                            "TVA " (document.issuer.vat_number.clone())
                        </address>
                    </header>
                    <div class="parties">
                        <div>
                            <h2 class="muted">"Facturé à"</h2>
                            <address>for line in &buyer { (line.clone()) <br> }</address>
                        </div>
                    </div>
                    <table>
                        <caption class="muted">"Détail de la facture"</caption>
                        <thead>
                            <tr>
                                <th scope="col">"Désignation"</th>
                                <th scope="col" class="num">"Quantité"</th>
                                <th scope="col" class="num">(price_heading)</th>
                                <th scope="col" class="num">(total_heading)</th>
                            </tr>
                        </thead>
                        <tbody>
                            for (label, quantity, unit_price, total) in &lines {
                                <tr>
                                    <th scope="row">(label.clone())</th>
                                    <td class="num">(quantity.clone())</td>
                                    <td class="num">(unit_price.clone())</td>
                                    <td class="num">(total.clone())</td>
                                </tr>
                            }
                        </tbody>
                    </table>
                    <table class="totals">
                        <tbody>
                            <tr><td>"Sous-total"</td><td class="num">(money(&document.subtotal))</td></tr>
                            <tr><td>"Frais de port"</td><td class="num">(money(&document.shipping_fee))</td></tr>
                            if document.handling_fee.is_positive() {
                                <tr><td>"Frais de dossier"</td><td class="num">(money(&document.handling_fee))</td></tr>
                            }
                            if let Some((label, amount)) = &discount {
                                <tr><td>(label.clone())</td><td class="num">"− " (amount.clone())</td></tr>
                            }
                            if let Some((base, vat)) = &totals_excl_vat {
                                <tr><td>"Total HT"</td><td class="num">(base.clone())</td></tr>
                                <tr><td>"TVA"</td><td class="num">(vat.clone())</td></tr>
                            }
                            <tr class="total"><td>(total_heading)</td><td class="num">(money(&document.total))</td></tr>
                        </tbody>
                    </table>
                    if !vat_lines.is_empty() && document.amounts_include_vat {
                        <table>
                            <caption class="muted">"TVA par taux"</caption>
                            <thead>
                                <tr>
                                    <th scope="col">"Taux"</th>
                                    <th scope="col" class="num">"Base HT"</th>
                                    <th scope="col" class="num">"TVA"</th>
                                    <th scope="col" class="num">"TTC"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for (rate, base, vat, total) in &vat_lines {
                                    <tr>
                                        <th scope="row">(rate.clone())</th>
                                        <td class="num">(base.clone())</td>
                                        <td class="num">(vat.clone())</td>
                                        <td class="num">(total.clone())</td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                    }
                    if let Some(mention) = document.regime_mention { <p>(mention)</p> }
                    if let Some(base) = &document.base_currency { <p>(base.mention())</p> }
                    if !credit_notes.is_empty() {
                        <table>
                            <caption class="muted">"Avoirs émis sur cette facture"</caption>
                            <thead>
                                <tr>
                                    <th scope="col">"Avoir"</th>
                                    <th scope="col">"Date"</th>
                                    <th scope="col">"Motif"</th>
                                    <th scope="col" class="num">"Montant"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for (number, issued, reason, amount) in &credit_notes {
                                    <tr>
                                        <th scope="row">(number.clone())</th>
                                        <td>(issued.clone())</td>
                                        <td>(reason.clone())</td>
                                        <td class="num">"− " (amount.clone())</td>
                                    </tr>
                                }
                            </tbody>
                        </table>
                        <p><strong>"Net après avoirs : " (money(&document.net_after_credit_notes))</strong></p>
                    }
                </main>
                <footer>
                    (document.issuer.name.clone()) " · " (document.issuer.registration.clone()) " · TVA " (document.issuer.vat_number.clone())
                    <br>
                    "Une question sur cette facture ? " (document.issuer.contact.clone())
                </footer>
            </body>
        </html>
    })
}
