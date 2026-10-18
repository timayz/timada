mod common;

use common::*;
use timada_support::{MessageStatus, ReceiveMessage, SupportError};

#[tokio::test]
async fn a_visitor_sends_a_message_as_written() {
    let support = command().await;
    let message = receive(&support, "  Where is my order?\nThanks.  ").await;
    assert_eq!(message.first_name, "Ada");
    assert_eq!(message.last_name, "Lovelace");
    assert_eq!(message.full_name(), "Ada Lovelace");
    assert_eq!(message.email, "ada@example.com");
    assert_eq!(message.body, "Where is my order?\nThanks.");
    assert_eq!(message.language.as_deref(), Some("fr"));
    assert_eq!(message.status, MessageStatus::Open);
    assert_eq!(message.handled_at, None);
    assert!(message.received_at > 0);

    // Every message is its own stream.
    let other = receive(&support, "Do you ship to Belgium?").await;
    assert_ne!(other.id, message.id);
    assert_eq!(
        support.load_message(&message.id).await.unwrap().unwrap(),
        message
    );
}

#[tokio::test]
async fn what_a_visitor_writes_is_checked() {
    let support = command().await;
    let with = |change: fn(&mut ReceiveMessage)| {
        let mut cmd = draft("Where is my order? It is late.");
        change(&mut cmd);
        cmd
    };
    for (cmd, code) in [
        (with(|c| c.first_name = "  ".into()), "message_invalid_name"),
        (
            with(|c| c.last_name = "x".repeat(81)),
            "message_invalid_name",
        ),
        (with(|c| c.email = "ada".into()), "message_invalid_email"),
        (with(|c| c.body = " short ".into()), "message_invalid_body"),
        (with(|c| c.body = "x".repeat(4001)), "message_invalid_body"),
    ] {
        assert_eq!(cmd.validate().unwrap_err().code(), Some(code));
        let err = support.receive_message(VISITOR, cmd).await.unwrap_err();
        assert_eq!(err.code(), Some(code));
        assert!(err.is_user_facing());
    }
    // An unreadable language is no language.
    let cmd = with(|c| c.language = Some("???".into()));
    let message = support.receive_message(VISITOR, cmd).await.unwrap();
    assert_eq!(message.language, None);
}

#[tokio::test]
async fn the_merchant_handles_and_reopens_a_message() {
    let support = command().await;
    let message = receive(&support, "Where is my order? It is late.").await;

    support.handle_message(ALICE, &message.id).await.unwrap();
    let handled = support.load_message(&message.id).await.unwrap().unwrap();
    assert_eq!(handled.status, MessageStatus::Handled);
    assert!(handled.is_handled());
    assert!(handled.handled_at.is_some());
    assert!(handled.updated_at >= message.updated_at);

    // Again is a no-op: nothing more is written.
    support.handle_message(ALICE, &message.id).await.unwrap();
    assert_eq!(
        support.load_message(&message.id).await.unwrap().unwrap(),
        handled
    );

    support.reopen_message(ALICE, &message.id).await.unwrap();
    let reopened = support.load_message(&message.id).await.unwrap().unwrap();
    assert_eq!(reopened.status, MessageStatus::Open);
    assert_eq!(reopened.handled_at, None);
    support.reopen_message(ALICE, &message.id).await.unwrap();
    assert_eq!(
        support.load_message(&message.id).await.unwrap().unwrap(),
        reopened
    );

    for result in [
        support.handle_message(ALICE, "nothing").await,
        support.reopen_message(ALICE, "nothing").await,
    ] {
        assert!(matches!(result, Err(SupportError::NotFound)));
    }
}
