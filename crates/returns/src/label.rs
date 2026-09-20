//! Prepaid return labels. Where they come from is a port,
//! [`ReturnLabelProvider`] — a carrier's API behind an adapter of the host's.
//! Without one, labels are **manual**: the operator buys the label on the
//! carrier's site and attaches it (a link, a file, or both) with
//! [`Command::issue_return_label`](crate::Command::issue_return_label).
//! Either way the return records `ReturnLabelIssued`, and the file — too big
//! for an event, nothing a replay needs — is kept in `return_label_file`.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use sqlx::SqlitePool;
use timada_core::Address;

/// A label's file may weigh this much.
pub const MAX_LABEL_FILE_BYTES: usize = 5 * 1024 * 1024;

/// What a label's file may be: it is served back to customers from the
/// shop's own origin, so nothing a browser would run.
pub const LABEL_CONTENT_TYPES: [&str; 3] = ["application/pdf", "image/png", "image/jpeg"];

/// The file of a label: what the customer prints.
#[derive(Clone, PartialEq, Eq)]
pub struct LabelFile {
    pub file_name: String,
    pub content_type: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for LabelFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LabelFile")
            .field("file_name", &self.file_name)
            .field("content_type", &self.content_type)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// What a provider is asked a label for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelRequest {
    pub return_id: String,
    /// "R2026-000042": what the warehouse reads on the parcel.
    pub rma_number: String,
    /// Where the parcel leaves from: the order's delivery address.
    pub sender: Address,
    /// Units coming back, all lines together.
    pub units: u32,
}

/// What a provider gives back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvidedLabel {
    pub carrier: String,
    pub tracking_number: String,
    pub url: Option<String>,
    pub file: Option<LabelFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LabelError {
    /// The carrier will not make this label (address it does not serve, …).
    #[error("refused: {0}")]
    Refused(String),
    /// The carrier could not be reached; asking again may work.
    #[error("unavailable: {0}")]
    Unavailable(String),
}

/// The result of a provider call, boxed so providers can be `dyn`.
pub type LabelFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, LabelError>> + Send + 'a>>;

pub trait ReturnLabelProvider: Send + Sync {
    /// Buys a label for the return. Called once per return: the label is
    /// recorded as soon as it comes back.
    fn label<'a>(&'a self, request: &'a LabelRequest) -> LabelFuture<'a, ProvidedLabel>;
}

/// The provider, in a shape pages can carry.
#[derive(Clone)]
pub struct ReturnLabels(pub Arc<dyn ReturnLabelProvider>);

impl ReturnLabels {
    pub fn new(provider: impl ReturnLabelProvider + 'static) -> Self {
        Self(Arc::new(provider))
    }
}

/// A provider for tests and demos: every label is a one-page stand-in PDF
/// with a tracking number made from the RMA number, unless an answer was
/// scripted.
#[derive(Default)]
pub struct FakeLabelProvider {
    answers: Mutex<Vec<Result<ProvidedLabel, LabelError>>>,
    asked: Mutex<Vec<LabelRequest>>,
}

impl FakeLabelProvider {
    /// The next call gets this answer instead of a label.
    pub fn answer(&self, answer: Result<ProvidedLabel, LabelError>) {
        if let Ok(mut answers) = self.answers.lock() {
            answers.push(answer);
        }
    }

    /// Every request made so far.
    pub fn asked(&self) -> Vec<LabelRequest> {
        self.asked.lock().map(|a| a.clone()).unwrap_or_default()
    }
}

impl ReturnLabelProvider for FakeLabelProvider {
    fn label<'a>(&'a self, request: &'a LabelRequest) -> LabelFuture<'a, ProvidedLabel> {
        Box::pin(async move {
            if let Ok(mut asked) = self.asked.lock() {
                asked.push(request.clone());
            }
            let scripted = self.answers.lock().ok().and_then(|mut a| {
                if a.is_empty() {
                    None
                } else {
                    Some(a.remove(0))
                }
            });
            if let Some(answer) = scripted {
                return answer;
            }
            Ok(ProvidedLabel {
                carrier: "Colissimo".to_owned(),
                tracking_number: format!("8R{}", request.rma_number.replace(['R', '-'], "")),
                url: None,
                file: Some(LabelFile {
                    file_name: format!("etiquette-{}.pdf", request.rma_number),
                    content_type: "application/pdf".to_owned(),
                    bytes: format!(
                        "%PDF-1.4\n% étiquette retour {}\n%%EOF\n",
                        request.rma_number
                    )
                    .into_bytes(),
                }),
            })
        })
    }
}

/// The file of a return's label, when its label is one.
pub async fn load_return_label_file(
    db: &SqlitePool,
    return_id: &str,
) -> sqlx::Result<Option<LabelFile>> {
    let row: Option<(String, String, Vec<u8>)> = sqlx::query_as(
        "SELECT file_name, content_type, content FROM return_label_file WHERE return_id = ?",
    )
    .bind(return_id)
    .fetch_optional(db)
    .await?;
    Ok(row.map(|(file_name, content_type, bytes)| LabelFile {
        file_name,
        content_type,
        bytes,
    }))
}
