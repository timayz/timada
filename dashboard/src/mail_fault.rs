//! Whether a mail could not be sent because of the address the customer
//! gave, as opposed to the server, the credentials or the network.
//!
//! A saga that sends mail records the first kind (`Undeliverable`) and moves
//! on; the second kind it records as deferred, for a later try by the
//! mailroom (#427, stage 3 of the CQRS plan; #464). [`SendFailure::of`]
//! tells the two apart, and [`attempt`] runs a send under the mailroom's
//! deadline and says which it was.

use std::error::Error as _;

use lettre::transport::smtp::Error as SmtpError;
use topcoat::mail::AddressError;

/// `true` when `err` says the recipient's address is at fault: it does not
/// parse ([`AddressError`]), or the server refused it for good with an SMTP
/// reply about the recipient (see [`recipient_refused`]). `false` for
/// everything else, authentication (535), policy (554, "relaying denied"),
/// transient replies (4xx), the network, timeouts and non-SMTP transports
/// included: those are worth a retry.
///
/// The SMTP reply is read from the `lettre` error that topcoat's
/// `SmtpTransport` and the shop transport (`shop_mail`) wrap in
/// `SendError::Delivery`, found by walking the error's chain. The error
/// must keep its chain: `?` into `topcoat::Error` and `.context(..)` do,
/// formatting it into a message does not. `lettre` is pinned to the 0.11
/// line topcoat-mail wraps: a topcoat upgrade that changes the wrapped type
/// fails this module's tests.
pub fn is_customer_address_fault(err: &topcoat::Error) -> bool {
    err.chain().any(|cause| {
        cause.is::<AddressError>()
            || cause
                .downcast_ref::<SmtpError>()
                .is_some_and(recipient_refused)
    })
}

/// How long of what the server said a stream keeps.
const REASON_LEN: usize = 200;

/// What a saga does with a send that failed.
#[derive(Debug)]
pub enum SendFailure {
    /// The recipient's address is at fault: record it, with what the
    /// server said, and move on.
    Undeliverable(String),
    /// Anything else: record it as not sent, for a later try.
    Retry(topcoat::Error),
}

/// How a send went, for the stream.
#[derive(Debug)]
pub enum Attempt {
    Sent,
    /// The recipient's address is at fault; what the server said.
    Undeliverable(String),
    /// The server, the credentials or the network are: what went wrong, in
    /// one line, for the stream and the cluster page.
    NotSent(String),
}

/// Runs `send` under the mailroom's deadline
/// ([`SEND_DEADLINE`](crate::mailroom::SEND_DEADLINE)) and says how it
/// went: a send still going at the deadline counts as not sent.
pub async fn attempt(send: impl Future<Output = topcoat::Result<()>>) -> Attempt {
    match tokio::time::timeout(crate::mailroom::SEND_DEADLINE, send).await {
        Ok(Ok(())) => Attempt::Sent,
        Ok(Err(err)) => match SendFailure::of(err) {
            SendFailure::Undeliverable(reason) => Attempt::Undeliverable(reason),
            SendFailure::Retry(err) => Attempt::NotSent(reason(&err)),
        },
        Err(_) => Attempt::NotSent(format!(
            "no answer within {} s",
            crate::mailroom::SEND_DEADLINE.as_secs()
        )),
    }
}

impl SendFailure {
    pub fn of(err: topcoat::Error) -> Self {
        if is_customer_address_fault(&err) {
            Self::Undeliverable(reason(&err))
        } else {
            Self::Retry(err)
        }
    }
}

/// What the server (or the address parser) said, in one line of at most
/// [`REASON_LEN`] characters: the innermost cause, which is the SMTP reply.
fn reason(err: &topcoat::Error) -> String {
    let cause = err
        .chain()
        .last()
        .map_or_else(|| err.to_string(), ToString::to_string);
    let line: String = cause.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(REASON_LEN) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// A permanent reply about the recipient: 550 (mailbox unavailable), 551
/// (user not local), 553 (mailbox name not allowed), and 501 (syntax) when
/// the enhanced status code says the recipient's address was the problem.
///
/// The enhanced status code (RFC 3463), when the server sends one, settles
/// what a reply is about, since 550 covers "no such user" as well as
/// "relaying denied": `5.1.x` is an address (`5.1.7` and `5.1.8` the
/// sender's own), `5.2.0` to `5.2.2` a mailbox that is gone, disabled or
/// full. Any other subject (policy, relaying, protocol, system) is not the
/// customer's doing, whatever the reply code. Without one, the reply code
/// alone decides.
fn recipient_refused(err: &SmtpError) -> bool {
    if !err.is_permanent() {
        return false;
    }
    let Some(code) = err.status() else {
        return false;
    };
    // lettre keeps the reply's text as the error's source.
    let message = err.source().map(ToString::to_string).unwrap_or_default();
    match (u16::from(code), enhanced_status(&message)) {
        (_, Some((1, 7 | 8))) => false,
        (550 | 551 | 553 | 501, Some((1, _))) => true,
        (550 | 551 | 553, Some((2, 0..=2))) => true,
        (550 | 551 | 553, None) => true,
        _ => false,
    }
}

/// The subject and detail of the enhanced status code a permanent reply
/// starts with: `"5.1.1 no such user"` gives `(1, 1)`, `"mailbox
/// unavailable"` gives `None`.
fn enhanced_status(message: &str) -> Option<(u16, u16)> {
    let word = message.split_whitespace().next()?;
    let mut parts = word.split('.');
    let class = parts.next()?;
    let subject = parts.next()?.parse().ok()?;
    let detail = parts.next()?.parse().ok()?;
    (class == "5" && parts.next().is_none()).then_some((subject, detail))
}

/// A mail server of our own, for the tests of the sagas: it answers every
/// `RCPT TO` with the reply it was given, and takes anything else.
#[cfg(test)]
pub(crate) mod fake_smtp {
    use std::{
        io::{BufRead as _, BufReader, Write as _},
        net::TcpListener,
        thread,
    };

    /// Serves one connection on a loopback port, answering `reply` to
    /// `RCPT TO`; returns the port.
    pub(crate) fn serve(reply: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("the port").port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("a connection");
            let mut reader = BufReader::new(stream.try_clone().expect("a reader"));
            stream
                .write_all(b"220 fake ESMTP\r\n")
                .expect("the greeting");
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let answer = if line.starts_with("RCPT TO") {
                    reply
                } else if line.starts_with("QUIT") {
                    "221 bye"
                } else {
                    "250 OK"
                };
                if stream
                    .write_all(format!("{answer}\r\n").as_bytes())
                    .is_err()
                    || line.starts_with("QUIT")
                {
                    break;
                }
            }
        });
        port
    }

    /// A port nothing listens on any more.
    pub(crate) fn closed_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        listener.local_addr().expect("the port").port()
    }

    /// A topcoat transport to the server at `port`, in clear.
    pub(crate) fn transport(port: u16) -> topcoat::mail::SmtpTransport {
        topcoat::mail::SmtpTransport::from_url(&format!("smtp://127.0.0.1:{port}"))
            .expect("a loopback address")
            .build()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use lettre::{Message, SmtpTransport, Transport as _};
    use topcoat::mail::{Mailbox, SendError};

    use super::*;

    /// A mail sent to a server of our own that answers `reply` to
    /// `RCPT TO`, with the failure as the transports hand it over: the
    /// lettre error inside `SendError::Delivery` inside a `topcoat::Error`.
    fn refused_with(reply: &'static str) -> topcoat::Error {
        let port = fake_smtp::serve(reply);
        let transport = SmtpTransport::builder_dangerous("127.0.0.1")
            .port(port)
            .timeout(Some(Duration::from_secs(5)))
            .build();
        let err = transport
            .send(&hello())
            .expect_err("the server refused the recipient");
        SendError::delivery(err).into()
    }

    fn hello() -> Message {
        Message::builder()
            .from("shop@example.org".parse().expect("the sender"))
            .to("ada@example.org".parse().expect("the recipient"))
            .subject("hello")
            .body(String::from("hi"))
            .expect("a message")
    }

    #[test]
    fn a_refused_recipient_is_the_customers_fault() {
        for reply in [
            "550 mailbox unavailable",
            "551 user not local",
            "553 mailbox name not allowed",
            "550 5.1.1 no such user",
            "550-5.1.1 no such user\r\n550 5.1.1 check the address",
            "550 5.2.1 mailbox disabled",
            "501 5.1.3 bad recipient address syntax",
        ] {
            assert!(is_customer_address_fault(&refused_with(reply)), "{reply}");
        }
    }

    #[test]
    fn any_other_reply_is_worth_a_retry() {
        for reply in [
            "535 5.7.8 authentication failed",
            "554 5.7.1 relaying denied",
            "550 5.7.1 relaying denied",
            "550 5.7.26 unauthenticated email is not accepted",
            "550 5.2.3 message too long",
            "501 syntax error in parameters",
            "501 5.1.7 bad sender address syntax",
            "450 4.2.0 try again later",
            "421 service not available",
        ] {
            assert!(!is_customer_address_fault(&refused_with(reply)), "{reply}");
        }
    }

    #[test]
    fn a_server_that_cannot_be_reached_is_not_the_customers_fault() {
        // Nothing listens there any more.
        let transport = SmtpTransport::builder_dangerous("127.0.0.1")
            .port(fake_smtp::closed_port())
            .timeout(Some(Duration::from_secs(5)))
            .build();
        let err = transport.send(&hello()).expect_err("nobody listens");
        let err: topcoat::Error = SendError::delivery(err).into();
        assert!(!is_customer_address_fault(&err));
    }

    #[test]
    fn an_address_that_does_not_parse_is_the_customers_fault() {
        let err: topcoat::Error = Mailbox::new("ada at example.org")
            .expect_err("not an address")
            .into();
        assert!(is_customer_address_fault(&err));
        // Through a context layer too: the chain is what counts.
        assert!(is_customer_address_fault(&err.context("sign-in code")));
    }

    #[test]
    fn a_transport_that_fails_on_its_own_is_not_the_customers_fault() {
        let spool = std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the mail spool is read-only",
        );
        let err: topcoat::Error = SendError::delivery(spool).into();
        assert!(!is_customer_address_fault(&err));
        assert!(!is_customer_address_fault(&topcoat::Error::msg(
            "no mailer configured"
        )));
    }

    #[test]
    fn a_failure_is_recorded_or_retried_with_what_the_server_said() {
        match SendFailure::of(refused_with("550 5.1.1 no such user")) {
            SendFailure::Undeliverable(reason) => {
                assert!(reason.contains("5.1.1 no such user"), "{reason}");
                assert!(reason.len() < 120, "{reason}");
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            SendFailure::of(refused_with("450 4.2.0 try again later")),
            SendFailure::Retry(_)
        ));
        let err: topcoat::Error = Mailbox::new("ada at example.org")
            .expect_err("not an address")
            .into();
        assert!(matches!(
            SendFailure::of(err.context("sign-in code")),
            SendFailure::Undeliverable(_)
        ));
        let long = reason(&topcoat::Error::msg("x".repeat(500)));
        assert_eq!(long.chars().count(), REASON_LEN + 1);
        assert!(long.ends_with('…'));
        assert_eq!(reason(&topcoat::Error::msg("two\n  lines")), "two lines");
    }

    #[test]
    fn the_enhanced_status_code_is_read_from_the_reply() {
        assert_eq!(enhanced_status("5.1.1 no such user"), Some((1, 1)));
        assert_eq!(enhanced_status("5.7.26 unauthenticated"), Some((7, 26)));
        assert_eq!(enhanced_status("mailbox unavailable"), None);
        assert_eq!(enhanced_status("4.2.0 try again"), None);
        assert_eq!(enhanced_status("5.1 short"), None);
        assert_eq!(enhanced_status("5.1.1.1 long"), None);
        assert_eq!(enhanced_status(""), None);
    }
}
