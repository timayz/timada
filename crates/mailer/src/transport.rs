//! Where e-mails go once the outbox hands them over.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use crate::{email::Email, error::MailError};

/// The result of [`Transport::send`], boxed so transports can be `dyn`.
pub type SendFuture<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

/// Delivers one e-mail. An `Err` leaves it in the outbox for a retry.
pub trait Transport: Send + Sync {
    fn send<'a>(&'a self, email: &'a Email) -> SendFuture<'a>;
}

/// Writes e-mails to the log instead of sending them: the default of a
/// development setup.
#[derive(Debug, Clone, Copy, Default)]
pub struct LogTransport;

impl Transport for LogTransport {
    fn send<'a>(&'a self, email: &'a Email) -> SendFuture<'a> {
        Box::pin(async move {
            tracing::info!(to = %email.to, subject = %email.subject, body = %email.body, html = email.html_body.is_some(), "e-mail (not sent: log transport)");
            Ok(())
        })
    }
}

/// Keeps e-mails in memory: tests read them back with [`Self::sent`].
#[derive(Debug, Clone, Default)]
pub struct MemoryTransport {
    sent: Arc<Mutex<Vec<Email>>>,
}

impl MemoryTransport {
    pub fn sent(&self) -> Vec<Email> {
        self.sent
            .lock()
            .map(|sent| sent.clone())
            .unwrap_or_default()
    }
}

impl Transport for MemoryTransport {
    fn send<'a>(&'a self, email: &'a Email) -> SendFuture<'a> {
        Box::pin(async move {
            self.sent
                .lock()
                .map_err(|_| MailError::Transport("memory transport poisoned".into()))?
                .push(email.clone());
            Ok(())
        })
    }
}

#[cfg(feature = "smtp")]
pub use smtp::SmtpTransport;

#[cfg(feature = "smtp")]
mod smtp {
    use lettre::{
        AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
        message::{MultiPart, header::ContentType},
    };

    use super::{SendFuture, Transport};
    use crate::{email::Email, error::MailError};

    /// Sends through an SMTP relay.
    #[derive(Clone)]
    pub struct SmtpTransport {
        inner: AsyncSmtpTransport<Tokio1Executor>,
    }

    impl SmtpTransport {
        /// From a connection URL such as `smtps://user:pass@smtp.example.com`
        /// or `smtp://localhost:1025` (see `lettre`'s `from_url`).
        pub fn from_url(url: &str) -> Result<Self, MailError> {
            let inner = AsyncSmtpTransport::<Tokio1Executor>::from_url(url)
                .map_err(|err| MailError::Transport(err.to_string()))?
                .build();
            Ok(Self { inner })
        }
    }

    impl Transport for SmtpTransport {
        fn send<'a>(&'a self, email: &'a Email) -> SendFuture<'a> {
            Box::pin(async move {
                let message = Message::builder()
                    .from(
                        email
                            .from
                            .parse()
                            .map_err(|_| MailError::InvalidMailbox(email.from.clone()))?,
                    )
                    .to(email
                        .to
                        .parse()
                        .map_err(|_| MailError::InvalidMailbox(email.to.clone()))?)
                    .subject(email.subject.clone());
                let message = match &email.html_body {
                    Some(html) => message.multipart(MultiPart::alternative_plain_html(
                        email.body.clone(),
                        html.clone(),
                    )),
                    None => message
                        .header(ContentType::TEXT_PLAIN)
                        .body(email.body.clone()),
                }
                .map_err(|err| MailError::Transport(err.to_string()))?;
                self.inner
                    .send(message)
                    .await
                    .map_err(|err| MailError::Transport(err.to_string()))?;
                Ok(())
            })
        }
    }
}
