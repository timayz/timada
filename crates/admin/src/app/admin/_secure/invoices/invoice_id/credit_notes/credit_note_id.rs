//! `/{mount}/invoices/{invoice_id}/credit-notes/{credit_note_id}`: one credit
//! note ("avoir") of an invoice — its PDF and the check of its archived file.
//! It has no page: the invoice's page shows it.

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        error::RouterErrorExt, error::see_other, href, page, path_param, path_param as param,
    },
    view::View,
};

#[cfg(feature = "pdf")]
use crate::{app::admin::_secure::invoices::invoice_id::PdfDownload, config::AdminConfig};
use crate::{
    app::admin::_secure::invoices::invoice_id::{self, InvoiceId, archive_check_code},
    config::AdminServices,
};

path_param!(pub credit_note_id: String, error = not_found);

/// The credit note of this invoice the path names; one of another invoice is
/// a 404.
async fn own_credit_note(cx: &Cx) -> Result<(String, timada_invoice::CreditNoteView)> {
    let id = param::<InvoiceId>(cx)?.clone();
    let note_id = param::<CreditNoteId>(cx)?.clone();
    let services = app_context::<AdminServices>(cx);
    let note = timada_invoice::load_credit_note(&services.executor, &note_id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|note| note.invoice_id == id)
        .ok_or_not_found()?;
    Ok((id, note))
}

/// `./pdf`: the credit note as a file — the archived one when the
/// shop has an archive, filed now if need be.
#[cfg(feature = "pdf")]
#[topcoat::router::route(GET "./pdf")]
pub async fn download(cx: &Cx) -> Result<PdfDownload> {
    let (_, note) = own_credit_note(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let issuer = &app_context::<AdminConfig>(cx).invoice_issuer;
    let document = timada_invoice::load_credit_note_document(&services.executor, issuer, &note.id)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .ok_or_not_found()?;
    let archived = match &services.archive {
        Some(archive) => timada_invoice::archive_credit_note(
            &services.executor,
            &services.db,
            archive.0.as_ref(),
            issuer,
            &note.id,
            &timada_invoice::ArchivePolicy::default(),
        )
        .await?
        .map(|(_, bytes)| bytes),
        None => None,
    };
    let bytes = match archived {
        Some(bytes) => bytes,
        None => timada_invoice::render_credit_note_pdf(&document)?,
    };
    Ok(PdfDownload {
        file_name: timada_invoice::credit_note_pdf_file_name(&document),
        bytes,
    })
}

/// Checks a credit note's archived file like the invoice's; the outcome comes
/// back on the invoice's page, under the credit notes.
#[page(POST "./verify")]
pub async fn verify(cx: &Cx) -> Result<impl View> {
    let (id, note) = own_credit_note(cx).await?;
    let services = app_context::<AdminServices>(cx);
    let outcome = match &services.archive {
        Some(archive) => {
            timada_invoice::verify_archived(&services.db, archive.0.as_ref(), &note.id).await?
        }
        None => None,
    };
    let target = href!(invoice_id::show, InvoiceId(id))
        .query([
            ("archive", archive_check_code(outcome)),
            ("avoir", note.credit_note_number.as_str()),
        ])
        .resolve(cx);
    Err::<(), _>(see_other(target).into())
}
