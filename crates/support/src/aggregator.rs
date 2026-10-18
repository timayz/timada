//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.

/// One message a visitor sent through the shop's contact form. Its id is a
/// fresh [`new_message_id`](crate::new_message_id).
#[evento::aggregate(name = "timada-support/Message")]
pub enum Message {
    /// Taken from the form, as the visitor wrote it (trimmed, the address
    /// lower-cased).
    Received {
        first_name: String,
        last_name: String,
        email: String,
        body: String,
        /// The primary subtag of the visitor's browser language (`fr`), to
        /// answer them in it.
        language: Option<String>,
        received_by: String,
    },
    /// The merchant answered it, or has nothing to do about it.
    Handled { handled_by: String },
    /// Back to waiting: the merchant marked it handled too soon.
    Reopened { reopened_by: String },
}
