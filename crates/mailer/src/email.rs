/// An e-mail ready for a [`crate::Transport`]: always a plain-text body, an
/// HTML alternative when the template wrote one, and the files that go with
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub html_body: Option<String>,
    pub attachments: Vec<Attachment>,
}

/// A file sent along with an e-mail.
#[derive(Clone, PartialEq, Eq)]
pub struct Attachment {
    pub file_name: String,
    /// Media type, e.g. `application/pdf`.
    pub content_type: String,
    pub content: Vec<u8>,
}

impl Attachment {
    pub fn pdf(file_name: impl Into<String>, content: Vec<u8>) -> Self {
        Self {
            file_name: file_name.into(),
            content_type: "application/pdf".to_owned(),
            content,
        }
    }
}

// The bytes of a file have no place in a log line.
impl std::fmt::Debug for Attachment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attachment")
            .field("file_name", &self.file_name)
            .field("content_type", &self.content_type)
            .field("bytes", &self.content.len())
            .finish()
    }
}
