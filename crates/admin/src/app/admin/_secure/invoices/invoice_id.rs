//! `/{mount}/invoices/{invoice_id}`: one invoice as it will be printed —
//! lines, fees, reduction and total — and the credit notes issued against
//! it, each with its own PDF. Read-only: orders drive its lifecycle, refunds
//! its credit notes.

use timada_core::Money;
use timada_invoice::{credit_notes_of_invoice, load_invoice, load_invoice_document};
use timada_order::order_numbers_by_ids;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::RouterErrorExt, error::see_other, href, page, path_param, path_param as param,
        query_params, query_params as query,
    },
    view::{View, view},
};

use self::credit_notes::credit_note_id::{self, CreditNoteId};
use super::invoice_status_badge;
use crate::{
    app::admin::_secure::{
        customers::customer_id,
        orders::order_id::{self, address_lines},
    },
    components::{
        button::{ButtonSize, ButtonVariant, button_variants},
        card::{card, card_content, card_header, card_title},
    },
    config::{AdminConfig, AdminServices},
    ui::{date, detail_grid, detail_main, fact, facts, link, money, page_header, vat_rate},
};

pub mod credit_notes;

path_param!(pub invoice_id: String, error = not_found);

/// One credit note as the invoice's page lists it.
struct CreditNoteLine {
    id: String,
    number: String,
    issued: String,
    reason: String,
    amount: String,
    /// When its file was archived, when the shop has an archive and it was.
    archived_on: Option<String>,
    /// Where its PDF is downloaded (feature `pdf`).
    file: Option<String>,
}

#[query_params(error = bad_request)]
struct ShowQuery {
    /// The outcome of "Vérifier": `intact`, `altered`, `missing`.
    archive: Option<String>,
    /// The number of the credit note that was verified, when it was one's
    /// file rather than the invoice's.
    avoir: Option<String>,
}

#[page]
pub async fn show(cx: &Cx) -> Result<impl View> {
    let id = param::<InvoiceId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    // What the archive holds of this invoice, when the shop has one.
    let archived = match &services.archive {
        Some(_) => timada_invoice::archived_document(&services.db, &id).await?,
        None => None,
    }
    .map(|entry| {
        (
            date(entry.archived_at.max(0) as u64),
            entry.sha256,
            format!("{} Ko", (entry.size + 1_023) / 1_024),
            entry.reconstituted,
        )
    });
    let has_archive = services.archive.is_some();
    let asked = query::<ShowQuery>(cx)?;
    let outcome = match asked.archive.as_deref() {
        Some("intact") => Some((
            false,
            "Le fichier archivé est intact : son empreinte est celle du jour de l'archivage.",
        )),
        Some("altered") => Some((
            true,
            "Le fichier archivé a été modifié : son empreinte n'est plus celle du jour de l'archivage.",
        )),
        Some("missing") => Some((
            true,
            "Le fichier archivé est introuvable dans le dépôt d'archives.",
        )),
        _ => None,
    };
    // The invoice's own file, or one of its credit notes'.
    let (checked, note_checked) = match (&asked.avoir, outcome) {
        (Some(number), Some((alarming, message))) => (
            None,
            Some((alarming, format!("Avoir {number} — {message}"))),
        ),
        (_, outcome) => (outcome, None),
    };
    let invoice = load_invoice(&services.executor, &id)
        .await?
        .ok_or_not_found()?;

    let title = match &invoice.invoice_number {
        Some(number) => format!("Facture {number}"),
        None => "Facture non numérotée".to_owned(),
    };
    let order_label = order_numbers_by_ids(&services.db, std::slice::from_ref(&invoice.order_id))
        .await?
        .remove(&invoice.order_id)
        .unwrap_or_else(|| invoice.order_id.clone());
    let order_link = href!(order_id::show, order_id::OrderId(invoice.order_id.clone())).resolve(cx);
    let customer_link = href!(
        customer_id::show,
        customer_id::CustomerId(invoice.customer_id.clone())
    )
    .resolve(cx);
    let mut credited = Money::zero(&invoice.total.currency);
    let mut credit_notes = Vec::new();
    for note in credit_notes_of_invoice(&services.db, &id).await? {
        let amount = Money::new(note.amount_minor, &note.currency);
        credited = credited.checked_add(&amount)?;
        let archived_on = match &services.archive {
            Some(_) => timada_invoice::archived_document(&services.db, &note.credit_note_id)
                .await?
                .map(|entry| date(entry.archived_at.max(0) as u64)),
            None => None,
        };
        #[cfg(feature = "pdf")]
        let file = Some(
            href!(
                credit_note_id::download,
                InvoiceId(id.clone()),
                CreditNoteId(note.credit_note_id.clone())
            )
            .resolve(cx),
        );
        #[cfg(not(feature = "pdf"))]
        let file: Option<String> = None;
        credit_notes.push(CreditNoteLine {
            id: note.credit_note_id,
            number: note.credit_note_number,
            issued: date(note.issued_at as u64),
            reason: timada_invoice::credit_reason_label(&note.reason),
            amount: money(&amount),
            archived_on,
            file,
        });
    }
    let net = money(&invoice.total.checked_sub(&credited)?);
    // What an invoice must show: the VAT per rate, or why there is none.
    let vat_lines: Vec<(String, String, String, String)> = invoice
        .tax
        .iter()
        .flat_map(|tax| &tax.vat_lines)
        .map(|line| {
            (
                vat_rate(line.rate_bp),
                money(&line.base),
                money(&line.vat),
                money(&line.total),
            )
        })
        .collect();
    let vat_mention = invoice.regime_mention();
    let mut lines = Vec::with_capacity(invoice.lines.len());
    for line in &invoice.lines {
        lines.push((
            line.label.clone(),
            line.quantity.to_string(),
            money(&line.unit_price),
            money(&line.total()?),
        ));
    }

    // The file itself needs the `pdf` feature; the printable page never does.
    #[cfg(feature = "pdf")]
    let pdf_link = Some(href!(download, InvoiceId(id.clone())).resolve(cx));
    #[cfg(not(feature = "pdf"))]
    let pdf_link: Option<String> = None;

    Ok(view! {
        page_header(
            title: &title,
            invoice_status_badge(status: invoice.status)
            if invoice.invoice_number.is_some() {
                <a href=(href!(print, InvoiceId(id.clone()))) class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))>"Version imprimable"</a>
                if let Some(pdf) = &pdf_link {
                    <a href=(pdf.clone()) class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))>"Télécharger le PDF"</a>
                }
            }
        )
        <p class="-mt-4 mb-6 font-mono text-xs text-muted-foreground">(id.clone())</p>

        detail_grid(
            detail_main(
                card(
                    card_header(card_title("Lignes"))
                    card_content(
                        <table class="w-full text-sm">
                            <thead>
                                <tr class="border-b border-border text-left text-muted-foreground">
                                    <th scope="col" class="py-2 font-normal">"Désignation"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Qté"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Prix unitaire TTC"</th>
                                    <th scope="col" class="py-2 text-right font-normal">"Total TTC"</th>
                                </tr>
                            </thead>
                            <tbody>
                                for (label, quantity, unit_price, total) in &lines {
                                    <tr class="border-b border-border">
                                        <td class="py-2">(label.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(quantity.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(unit_price.clone())</td>
                                        <td class="py-2 text-right tabular-nums">(total.clone())</td>
                                    </tr>
                                }
                            </tbody>
                            <tfoot>
                                <tr><td colspan="3" class="pt-3 text-muted-foreground">"Sous-total"</td><td class="pt-3 text-right tabular-nums">(money(&invoice.subtotal))</td></tr>
                                <tr><td colspan="3" class="text-muted-foreground">"Frais de port"</td><td class="text-right tabular-nums">(money(&invoice.shipping_fee))</td></tr>
                                <tr><td colspan="3" class="text-muted-foreground">"Frais de dossier"</td><td class="text-right tabular-nums">(money(&invoice.handling_fee))</td></tr>
                                if let Some(discount) = &invoice.discount {
                                    <tr><td colspan="3" class="text-muted-foreground">(discount.label.clone())</td><td class="text-right tabular-nums">"− " (money(&discount.amount))</td></tr>
                                }
                                <tr class="font-semibold"><td colspan="3" class="pt-2">"Total TTC"</td><td class="pt-2 text-right tabular-nums">(money(&invoice.total))</td></tr>
                            </tfoot>
                        </table>
                    )
                )
                if !vat_lines.is_empty() {
                    card(
                        card_header(card_title("TVA"))
                        card_content(
                            <table class="w-full text-sm">
                                <thead>
                                    <tr class="border-b border-border text-left text-muted-foreground">
                                        <th scope="col" class="py-2 font-normal">"Taux"</th>
                                        <th scope="col" class="py-2 text-right font-normal">"Base HT"</th>
                                        <th scope="col" class="py-2 text-right font-normal">"TVA"</th>
                                        <th scope="col" class="py-2 text-right font-normal">"TTC"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    for (rate, base, vat, total) in &vat_lines {
                                        <tr class="border-b border-border last:border-0">
                                            <td class="py-2">(rate.clone())</td>
                                            <td class="py-2 text-right tabular-nums">(base.clone())</td>
                                            <td class="py-2 text-right tabular-nums">(vat.clone())</td>
                                            <td class="py-2 text-right tabular-nums">(total.clone())</td>
                                        </tr>
                                    }
                                </tbody>
                            </table>
                            if let Some(mention) = vat_mention { <p class="mt-3 text-xs text-muted-foreground">(mention)</p> }
                        )
                    )
                }
                if !credit_notes.is_empty() {
                    card(
                        card_header(card_title("Avoirs"))
                        card_content(
                            <table class="w-full text-sm">
                                <thead>
                                    <tr class="border-b border-border text-left text-muted-foreground">
                                        <th scope="col" class="py-2 font-normal">"Numéro"</th>
                                        <th scope="col" class="py-2 font-normal">"Date"</th>
                                        <th scope="col" class="py-2 font-normal">"Motif"</th>
                                        <th scope="col" class="py-2 text-right font-normal">"Montant"</th>
                                        <th scope="col" class="py-2 pl-4 font-normal">"Document"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    for note in &credit_notes {
                                        <tr class="border-b border-border">
                                            <td class="py-2 font-mono text-xs">(note.number.clone())</td>
                                            <td class="py-2">(note.issued.clone())</td>
                                            <td class="py-2">(note.reason.clone())</td>
                                            <td class="py-2 text-right tabular-nums">"− " (note.amount.clone())</td>
                                            <td class="py-2 pl-4 text-xs">
                                                if let Some(file) = &note.file {
                                                    <a href=(file.clone()) aria-label=(format!("Télécharger l'avoir {} (PDF)", note.number)) class="underline underline-offset-4">"PDF"</a>
                                                }
                                                if let Some(archived_on) = &note.archived_on {
                                                    <span class="block text-muted-foreground">"Archivé le " (archived_on.clone())</span>
                                                    <form method="post" action=(href!(credit_note_id::verify, InvoiceId(id.clone()), CreditNoteId(note.id.clone())))>
                                                        <button type="submit" aria-label=(format!("Vérifier le fichier archivé de l'avoir {}", note.number)) class="underline underline-offset-4">"Vérifier"</button>
                                                    </form>
                                                }
                                            </td>
                                        </tr>
                                    }
                                </tbody>
                                <tfoot>
                                    <tr class="font-semibold"><td colspan="3" class="pt-2">"Net après avoirs"</td><td class="pt-2 text-right tabular-nums">(net.clone())</td><td></td></tr>
                                </tfoot>
                            </table>
                            if let Some((alarming, message)) = &note_checked {
                                <p role=(if *alarming { "alert" } else { "status" }) class=(if *alarming { "mt-3 text-sm text-destructive" } else { "mt-3 text-sm" })>(message.clone())</p>
                            }
                        )
                    )
                }
            )

            detail_main(
                if has_archive {
                    card(
                        card_header(card_title("Archive"))
                        card_content(
                            match &archived {
                                Some((archived_on, sha256, size, reconstituted)) => {
                                    facts(
                                        fact(term: "Archivée le", (archived_on.clone()) " · " (size.clone()))
                                        fact(term: "Empreinte SHA-256", class: "break-all font-mono text-xs", (sha256.clone()))
                                    )
                                    if *reconstituted {
                                        <p class="mt-2 text-sm text-muted-foreground">"Reconstituée : archivée longtemps après son émission, avec l'émetteur et la mise en page du jour de l'archivage. Figée depuis."</p>
                                    }
                                    if let Some((alarming, message)) = checked {
                                        <p role=(if alarming { "alert" } else { "status" }) class=(if alarming { "mt-3 text-sm text-destructive" } else { "mt-3 text-sm" })>(message)</p>
                                    }
                                    <form method="post" action=(href!(verify, InvoiceId(id.clone()))) class="mt-3">
                                        <button type="submit" class="text-sm underline underline-offset-4">"Vérifier le fichier archivé"</button>
                                    </form>
                                }
                                None => { <p class="text-sm text-muted-foreground">"Pas encore archivée : seule une facture émise l'est, quelques instants après son émission."</p> }
                            }
                        )
                    )
                }
                card(
                    card_header(card_title("Facturé à"))
                    card_content(
                        <div class="text-sm">
                            if let Some(company) = &invoice.company {
                                <span class="block font-medium">(company.company_name.clone())</span>
                                <span class="block font-mono text-xs">(company.vat_number.clone())</span>
                            }
                            address_lines(address: &invoice.billing_address)
                        </div>
                    )
                )
                card(
                    card_header(card_title("Références"))
                    card_content(
                        facts(
                            fact(term: "Commande", link(href: order_link, class: "font-mono text-xs", (order_label)))
                            fact(term: "Client", link(href: customer_link, class: "font-mono text-xs", (invoice.customer_id.clone())))
                            if let Some(reason) = &invoice.voided_reason {
                                fact(term: "Motif d'annulation", (reason.clone()))
                            }
                        )
                    )
                )
            )
        )
    })
}

/// The invoice as the customer gets it, laid out for paper: the admin's
/// header is hidden in print, so the browser's print dialog gives the PDF.
/// Only an issued invoice has one.
#[page("./print")]
pub async fn print(cx: &Cx) -> Result<impl View> {
    let id = param::<InvoiceId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let issuer = &app_context::<AdminConfig>(cx).invoice_issuer;
    let document = load_invoice_document(&services.executor, &services.db, issuer, &id)
        .await?
        .ok_or_not_found()?;

    let title = format!("Facture {}", document.number);
    let company_lines = document.company_lines();
    let back = href!(show, InvoiceId(id.clone())).resolve(cx);
    let (price_heading, total_heading) = if document.amounts_include_vat {
        ("Prix unitaire TTC", "Total TTC")
    } else {
        ("Prix unitaire HT", "Total HT")
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
    let vat_lines: Vec<(String, String, String)> = document
        .vat_lines
        .iter()
        .filter(|_| document.amounts_include_vat)
        .map(|l| (vat_rate(l.rate_bp), money(&l.base), money(&l.vat)))
        .collect();
    let discount = document
        .discount
        .as_ref()
        .map(|(label, amount)| (label.clone(), money(amount)));
    let credit_notes: Vec<(String, String, String)> = document
        .credit_notes
        .iter()
        .map(|n| (n.number.clone(), date(n.issued_at), money(&n.amount)))
        .collect();

    Ok(view! {
        <div class="mb-6 flex items-center gap-3 text-sm print:hidden">
            <button type="button" onclick="window.print()" class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))>"Imprimer ou enregistrer en PDF"</button>
            <a href=(back) class="text-muted-foreground underline-offset-4 hover:underline">"Retour à la facture"</a>
        </div>
        <article class="mx-auto max-w-3xl text-sm">
            <header class="flex items-start justify-between gap-8 border-b-2 border-foreground pb-4">
                <div>
                    <h1 class="text-2xl font-semibold tracking-tight">(title)</h1>
                    <p class="mt-1 text-muted-foreground">"Date : " (date(document.issued_at)) " · Commande : " (document.order_label.clone())</p>
                </div>
                <address class="text-right not-italic">
                    <strong>(document.issuer.name.clone())</strong>
                    for line in &document.issuer.address_lines { <span class="block">(line.clone())</span> }
                    <span class="block">(document.issuer.registration.clone())</span>
                    <span class="block">"TVA " (document.issuer.vat_number.clone())</span>
                </address>
            </header>
            <section class="my-6">
                <h2 class="text-muted-foreground">"Facturé à"</h2>
                for line in &company_lines { <span class="block">(line.clone())</span> }
                address_lines(address: &document.buyer)
            </section>
            <table class="w-full">
                <thead>
                    <tr class="border-b border-border text-left text-muted-foreground">
                        <th scope="col" class="py-2 font-normal">"Désignation"</th>
                        <th scope="col" class="py-2 text-right font-normal">"Qté"</th>
                        <th scope="col" class="py-2 text-right font-normal">(price_heading)</th>
                        <th scope="col" class="py-2 text-right font-normal">(total_heading)</th>
                    </tr>
                </thead>
                <tbody>
                    for (label, quantity, unit_price, total) in &lines {
                        <tr class="border-b border-border">
                            <td class="py-2">(label.clone())</td>
                            <td class="py-2 text-right tabular-nums">(quantity.clone())</td>
                            <td class="py-2 text-right tabular-nums">(unit_price.clone())</td>
                            <td class="py-2 text-right tabular-nums">(total.clone())</td>
                        </tr>
                    }
                </tbody>
            </table>
            <dl class="ml-auto mt-4 grid w-72 grid-cols-2 gap-y-1">
                <dt class="text-muted-foreground">"Sous-total"</dt><dd class="text-right tabular-nums">(money(&document.subtotal))</dd>
                <dt class="text-muted-foreground">"Frais de port"</dt><dd class="text-right tabular-nums">(money(&document.shipping_fee))</dd>
                if document.handling_fee.is_positive() {
                    <dt class="text-muted-foreground">"Frais de dossier"</dt><dd class="text-right tabular-nums">(money(&document.handling_fee))</dd>
                }
                if let Some((label, amount)) = &discount {
                    <dt class="text-muted-foreground">(label.clone())</dt><dd class="text-right tabular-nums">"− " (amount.clone())</dd>
                }
                for (rate, base, vat) in &vat_lines {
                    <dt class="text-muted-foreground">"TVA " (rate.clone()) " sur " (base.clone())</dt><dd class="text-right tabular-nums">(vat.clone())</dd>
                }
                <dt class="border-t border-foreground pt-1 font-semibold">(total_heading)</dt><dd class="border-t border-foreground pt-1 text-right font-semibold tabular-nums">(money(&document.total))</dd>
                for (number, issued, amount) in &credit_notes {
                    <dt class="text-muted-foreground">"Avoir " (number.clone()) " du " (issued.clone())</dt><dd class="text-right tabular-nums">"− " (amount.clone())</dd>
                }
                if !credit_notes.is_empty() {
                    <dt class="font-semibold">"Net après avoirs"</dt><dd class="text-right font-semibold tabular-nums">(money(&document.net_after_credit_notes))</dd>
                }
            </dl>
            if let Some(mention) = document.regime_mention { <p class="mt-6">(mention)</p> }
            if let Some(base) = &document.base_currency { <p class="mt-3">(base.mention())</p> }
            <footer class="mt-8 border-t border-border pt-3 text-xs text-muted-foreground">
                (document.issuer.name.clone()) " · " (document.issuer.registration.clone()) " · TVA " (document.issuer.vat_number.clone()) " · " (document.issuer.contact.clone())
            </footer>
        </article>
    })
}

/// A PDF handed to the browser as a file to keep.
#[cfg(feature = "pdf")]
pub struct PdfDownload {
    file_name: String,
    bytes: Vec<u8>,
}

#[cfg(feature = "pdf")]
impl topcoat::router::response::IntoResponse for PdfDownload {
    fn into_response(self, _cx: &Cx) -> Result<topcoat::router::response::Response> {
        Ok(topcoat::router::response::Response::builder()
            .header("Content-Type", "application/pdf")
            .header(
                "Content-Disposition",
                format!("attachment; filename=\"{}\"", self.file_name),
            )
            .header("Cache-Control", "private, no-store")
            .body(topcoat::router::Body::from(self.bytes))?)
    }
}

/// `./pdf`: the same document as a file, rendered by the server — what the
/// customer downloads from their account.
#[cfg(feature = "pdf")]
#[topcoat::router::route(GET "./pdf")]
pub async fn download(cx: &Cx) -> Result<PdfDownload> {
    let id = param::<InvoiceId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let issuer = &app_context::<AdminConfig>(cx).invoice_issuer;
    let document = load_invoice_document(&services.executor, &services.db, issuer, &id)
        .await?
        .ok_or_not_found()?;
    // The archived file when the shop has an archive — filed now if need be —
    // so the operator downloads what the customer does.
    let archived = match &services.archive {
        Some(archive) => timada_invoice::archive_invoice(
            &services.executor,
            &services.db,
            archive.0.as_ref(),
            issuer,
            &id,
            &timada_invoice::ArchivePolicy::default(),
        )
        .await
        .map_err(anyhow::Error::from)?
        .map(|(_, bytes)| bytes),
        None => None,
    };
    let bytes = match archived {
        Some(bytes) => bytes,
        None => timada_invoice::render_invoice_pdf(&document).map_err(anyhow::Error::from)?,
    };
    Ok(PdfDownload {
        file_name: timada_invoice::invoice_pdf_file_name(&document),
        bytes,
    })
}

/// Re-reads the archived file and compares it with the hash taken when it was
/// filed; the outcome comes back on the invoice's page.
#[page(POST "./verify")]
pub async fn verify(cx: &Cx) -> Result<impl View> {
    let id = param::<InvoiceId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let outcome = match &services.archive {
        Some(archive) => timada_invoice::verify_archived(&services.db, archive.0.as_ref(), &id)
            .await
            .map_err(anyhow::Error::from)?,
        None => None,
    };
    let target = href!(show, InvoiceId(id))
        .query([("archive", archive_check_code(outcome))])
        .resolve(cx);
    Err::<(), _>(see_other(target).into())
}

fn archive_check_code(outcome: Option<timada_invoice::ArchiveCheck>) -> &'static str {
    match outcome {
        Some(timada_invoice::ArchiveCheck::Intact) => "intact",
        Some(timada_invoice::ArchiveCheck::Altered) => "altered",
        Some(timada_invoice::ArchiveCheck::Missing) => "missing",
        None => "",
    }
}
