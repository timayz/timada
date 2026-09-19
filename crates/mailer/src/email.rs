/// A plain-text e-mail ready for a [`crate::Transport`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub body: String,
}
