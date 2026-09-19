/// An e-mail ready for a [`crate::Transport`]: always a plain-text body, and
/// an HTML alternative when the template wrote one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
    pub html_body: Option<String>,
}
