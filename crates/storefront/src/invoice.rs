//! The invoice and the credit notes of an order, read from the store, and
//! the PDF a shopper downloads. The merchant's dashboard issues them a
//! moment after the payment (or the refund): an order just placed may have
//! none yet.

use timada_invoicing::{Command as Invoicing, IssuedDocument};
use topcoat_core::context::Cx;
use topcoat_core::error::{Error, Result};

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
    fn invoicing(&self) -> Invoicing<evento::Evento> {
        Invoicing(self.executor().clone())
    }

    /// See [`documents`].
    pub async fn documents(&self, order_id: &str) -> anyhow::Result<Vec<IssuedDocument>> {
        let Some(order) = self.order(order_id).await? else {
            return Ok(Vec::new());
        };
        let invoicing = self.invoicing();
        let mut documents = Vec::new();
        documents.extend(invoicing.load_invoice_of_order(order_id).await?);
        for refund_id in &order.refund_ids {
            documents.extend(invoicing.load_credit_note_of_refund(refund_id).await?);
        }
        Ok(documents)
    }

    /// See [`document_pdf`].
    pub async fn document_pdf(
        &self,
        order_id: &str,
        document_id: &str,
    ) -> anyhow::Result<Option<DocumentPdf>> {
        let Some(document) = self.invoicing().load_document(document_id).await? else {
            return Ok(None);
        };
        // A document id found under another order's address is not this
        // order's to show.
        if document.order_id != order_id {
            return Ok(None);
        }
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
