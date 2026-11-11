//! The invoice and the credit notes of an order, and the PDF a shopper
//! downloads. The merchant's dashboard issues them a moment after the
//! payment (or the refund): an order just placed may have none yet.
//!
//! Like the order itself, they are not read from the store: an invoice
//! series is every buyer's name, address and e-mail, so a storefront's key
//! may not read it, and the dashboard answers one order's documents over
//! HTTP (`GET /store/orders/{order_id}/documents`, see [`crate::cart`]).
//! The storefront draws the PDF from what it was answered.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use timada_invoicing::IssuedDocument;
use topcoat_core::context::Cx;
use topcoat_core::error::Result;
use topcoat_router::error::too_many_requests;
use topcoat_router::request::client_ip;

use crate::cart::NO_BODY;
use crate::context::store;
use crate::error::into_error;
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
        let bytes = timada_invoicing::pdf::render_cached(document)
            .await?
            .to_vec();
        Ok(Some(DocumentPdf { file_name, bytes }))
    }
}

/// The invoice of an order, then its credit notes, as far as they are
/// issued. Like [`order`](crate::order), anyone holding the order id can
/// read them: the host decides who may.
pub async fn documents(cx: &Cx, order_id: &str) -> Result<Vec<IssuedDocument>> {
    store(cx).documents(order_id).await.map_err(into_error)
}

/// How many PDFs one address may open within [`PDF_WINDOW`]: far more than
/// a shopper reads, few enough that a script cannot keep the storefront
/// drawing them. A document opened again comes from memory; this bounds the
/// asking itself, each one a call to the dashboard.
const PDFS_PER_ADDRESS: usize = 30;
const PDF_WINDOW: Duration = Duration::from_secs(600);

/// Past this many addresses, a call also forgets those that opened nothing
/// within the window.
const SWEEP_ABOVE: usize = 10_000;

/// Whether `ip` may open one more PDF now, counting it when it may; the
/// seconds until it may again when it may not.
fn allow_pdf(ip: IpAddr, now: Instant) -> Result<(), u64> {
    static OPENED: OnceLock<Mutex<HashMap<IpAddr, VecDeque<Instant>>>> = OnceLock::new();
    let mut opened = OPENED
        .get_or_init(Default::default)
        .lock()
        .expect("pdf limit poisoned");
    if opened.len() > SWEEP_ABOVE {
        opened.retain(|_, times| {
            times
                .back()
                .is_some_and(|last| now.duration_since(*last) < PDF_WINDOW)
        });
    }
    let times = opened.entry(ip).or_default();
    while times
        .front()
        .is_some_and(|first| now.duration_since(*first) >= PDF_WINDOW)
    {
        times.pop_front();
    }
    if times.len() >= PDFS_PER_ADDRESS {
        let first = *times.front().expect("a full window");
        let wait = PDF_WINDOW.saturating_sub(now.duration_since(first));
        return Err(wait.as_secs().max(1));
    }
    times.push_back(now);
    Ok(())
}

/// One of an order's documents as PDF; `None` when it is not one of that
/// order's (or not issued). An address opening more than
/// [`PDFS_PER_ADDRESS`] within ten minutes is answered 429.
pub async fn document_pdf(
    cx: &Cx,
    order_id: &str,
    document_id: &str,
) -> Result<Option<DocumentPdf>> {
    if let Some(ip) = client_ip(cx)
        && let Err(wait) = allow_pdf(ip, Instant::now())
    {
        return Err(too_many_requests(wait).into());
    }
    store(cx)
        .document_pdf(order_id, document_id)
        .await
        .map_err(into_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_opens_so_many_pdfs_per_window() {
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let other: IpAddr = "203.0.113.10".parse().unwrap();
        let start = Instant::now();
        for _ in 0..PDFS_PER_ADDRESS {
            assert!(allow_pdf(ip, start).is_ok());
        }
        assert_eq!(allow_pdf(ip, start), Err(PDF_WINDOW.as_secs()));
        assert!(allow_pdf(other, start).is_ok(), "each address its own");
        assert!(
            allow_pdf(ip, start + PDF_WINDOW).is_ok(),
            "the window moved"
        );
    }
}
