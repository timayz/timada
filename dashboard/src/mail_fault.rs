//! Whether a mail could not be sent because of the address the customer
//! gave, as opposed to the server, the credentials or the network.
//!
//! A saga that sends mail records the first kind (`Undeliverable`) and moves
//! on; it returns the second kind, so the saga retries under its supervisor
//! (#427, stage 3 of the CQRS plan). Nothing calls this before the mail
//! sagas land.

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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "called by the mail sagas (#427, stage 3)")
)]
pub fn is_customer_address_fault(err: &topcoat::Error) -> bool {
    err.chain().any(|cause| {
        cause.is::<AddressError>()
            || cause
                .downcast_ref::<SmtpError>()
                .is_some_and(recipient_refused)
    })
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

#[cfg(test)]
mod tests {
    use std::{
        io::{BufRead as _, BufReader, Write as _},
        net::TcpListener,
        thread,
        time::Duration,
    };

    use lettre::{Message, SmtpTransport, Transport as _};
    use topcoat::mail::{Mailbox, SendError};

    use super::*;

    /// A mail sent to a server of our own that answers `reply` to
    /// `RCPT TO`, with the failure as the transports hand it over: the
    /// lettre error inside `SendError::Delivery` inside a `topcoat::Error`.
    fn refused_with(reply: &'static str) -> topcoat::Error {
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
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
            listener.local_addr().expect("the port").port()
        };
        // Nothing listens there any more.
        let transport = SmtpTransport::builder_dangerous("127.0.0.1")
            .port(port)
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
