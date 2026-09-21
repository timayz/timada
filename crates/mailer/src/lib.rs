//! Transactional e-mails. Other contexts record facts ("order placed",
//! "alert triggered"); this crate turns the ones a customer should hear about
//! into e-mails.
//!
//! Nothing is sent from a subscription handler. Handlers only *enqueue* into
//! a SQL outbox, keyed by the event they react to so a redelivery never
//! produces a second e-mail; a delivery worker then hands the queue to a
//! [`Transport`] and retries what fails. Swapping the transport — logging,
//! in-memory, SMTP — never touches the rest.

mod config;
mod email;
mod error;
mod migration;
mod outbox;
mod process;
mod template;
mod templates;
mod transport;

pub use config::{GuestOrderLinks, MailerConfig};
pub use email::{Attachment, Email};
pub use error::MailError;
pub use migration::migrations;
pub use outbox::*;
pub use process::{MAILER_SUBSCRIPTION, mailer_subscription};
pub use templates::{Content, FrenchTemplates, MailerTemplates, Templates};
pub use transport::*;
