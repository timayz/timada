//! The persisted shapes. Every variant is frozen once merged (`events.lock`):
//! a change of shape is a new variant, never a changed field. Each event
//! carries who did it, so the stream is its own audit trail. No field is an
//! enum, so the lock never freezes one.
//!
//! What the visitor wrote is personal data, and events cannot be erased: it
//! is recorded sealed under a key of the message's own
//! ([`timada_core::Key`]), which [`Message::Forgotten`] destroys.

/// One message a visitor sent through the shop's contact form. Its id is a
/// fresh [`new_message_id`](crate::new_message_id).
#[evento::aggregate(name = "timada-support/Message")]
pub enum Message {
    /// Taken from the form, as the visitor wrote it (trimmed, the address
    /// lower-cased), each field sealed under the message's key, bound to the
    /// message id, the key id and the field's name.
    Received {
        /// Names the key the fields are sealed under, kept outside the store.
        key_id: String,
        first_name: Vec<u8>,
        last_name: Vec<u8>,
        email: Vec<u8>,
        body: Vec<u8>,
        /// The primary subtag of the visitor's browser language (`fr`), to
        /// answer them in it. Not personal data: kept in clear.
        language: Option<String>,
        received_by: String,
    },
    /// The merchant answered it, or has nothing to do about it.
    Handled { handled_by: String },
    /// Back to waiting: the merchant marked it handled too soon.
    Reopened { reopened_by: String },
    /// The merchant erased it (the visitor asked to be forgotten): its key is
    /// destroyed, which leaves what `Received` holds unreadable for good.
    Forgotten {
        key_id: String,
        forgotten_by: String,
    },
}
