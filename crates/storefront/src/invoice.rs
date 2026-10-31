//! The invoice and the credit notes of an order, and the PDF a shopper
//! downloads. The merchant's dashboard issues them a moment after the
//! payment (or the refund): an order just placed may have none yet.
//!
//! Like the order itself, they are not read from the store: an invoice
//! series is every buyer's name, address and e-mail, so a storefront's key
//! may not read it, and the dashboard answers one order's documents over
//! HTTP (`GET /store/orders/{order_id}/documents`, see [`crate::cart`]).
//! The storefront draws the PDF from what it was answered.

use timada_invoicing::IssuedDocument;
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

use crate::cart::NO_BODY;
use crate::context::store;
use crate::store::Store;

pub use timada_invoicing::{DocumentKind, format_amount_fr};

/// A document drawn as PDF, with the file name a browser saves it under.
pub struct DocumentPdf {
    /// `F2026-000042.pdf`.
    pub file_name: String,
    pub bytes: Vec<u8>,
}

impl Store {
    /// See [`documents`].
    pub async fn documents(&self, order_id: &str) -> anyhow::Result<Vec<IssuedDocument>> {
        match self
            .cart_call(
                reqwest::Method::GET,
                &format!("orders/{order_id}/documents"),
                NO_BODY,
            )
            .await?
            .parsed::<Vec<IssuedDocument>>()?
        {
            Some(Ok(documents)) => Ok(documents),
            Some(Err(err)) => Err(err.into()),
            // An order the shop has not has no documents.
            None => Ok(Vec::new()),
        }
    }

    /// See [`document_pdf`].
    pub async fn document_pdf(
        &self,
        order_id: &str,
        document_id: &str,
    ) -> anyhow::Result<Option<DocumentPdf>> {
        // Only this order's documents are asked for, so a document id from
        // another order is simply not among them.
        let Some(document) = self
            .documents(order_id)
            .await?
            .into_iter()
            .find(|document| document.document_id == document_id)
        else {
            return Ok(None);
        };
        let file_name = timada_invoicing::pdf::file_name(&document);
        let bytes =
            tokio::task::spawn_blocking(move || timada_invoicing::pdf::render(&document)).await??;
        Ok(Some(DocumentPdf { file_name, bytes }))
    }
}

/// The invoice of an order, then its credit notes, as far as they are
/// issued. Like [`order`](crate::order), anyone holding the order id can
/// read them: the host decides who may.
pub async fn documents(cx: &Cx, order_id: &str) -> Result<Vec<IssuedDocument>> {
    store(cx)
        .documents(order_id)
        .await
        .map_err(Error::from_anyhow)
}

/// One of an order's documents as PDF; `None` when it is not one of that
/// order's (or not issued).
pub async fn document_pdf(
    cx: &Cx,
    order_id: &str,
    document_id: &str,
) -> Result<Option<DocumentPdf>> {
    store(cx)
        .document_pdf(order_id, document_id)
        .await
        .map_err(Error::from_anyhow)
}
