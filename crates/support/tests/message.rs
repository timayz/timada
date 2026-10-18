mod common;

use common::*;
use timada_core::Keys;
use timada_support::{MessageStatus, ReceiveMessage, SupportError};

fn holds(sealed: &[u8], plain: &str) -> bool {
    sealed.windows(plain.len()).any(|w| w == plain.as_bytes())
}

#[tokio::test]
async fn a_visitor_sends_a_message_as_written() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "  Where is my order?\nThanks.  ").await;
    assert_eq!(message.first_name, "Ada");
    assert_eq!(message.last_name, "Lovelace");
    assert_eq!(message.full_name(), "Ada Lovelace");
    assert_eq!(message.email, "ada@example.com");
    assert_eq!(message.body, "Where is my order?\nThanks.");
    assert_eq!(message.language.as_deref(), Some("fr"));
    assert_eq!(message.status, MessageStatus::Open);
    assert_eq!(message.handled_at, None);
    assert!(message.received_at > 0);

    // The store holds nothing of it in clear, and the key opens it.
    let state = support.load_message(&message.id).await.unwrap().unwrap();
    assert!(keys.has(&state.key_id));
    assert!(!holds(&state.first_name, "Ada"));
    assert!(!holds(&state.email, "ada@example.com"));
    assert!(!holds(&state.body, "Where is my order?"));
    assert_eq!(state.language.as_deref(), Some("fr"));
    assert_eq!(state.open_with(&keys).await.unwrap().unwrap(), message);

    // Every message is its own stream, under its own key.
    let other = receive(&support, &keys, "Do you ship to Belgium?").await;
    assert_ne!(other.id, message.id);
    let other_state = support.load_message(&other.id).await.unwrap().unwrap();
    assert_ne!(other_state.key_id, state.key_id);
    assert_eq!(keys.len(), 2);
}

#[tokio::test]
async fn a_message_needs_its_key_kept_first() {
    let support = command().await;
    let keys = MemoryKeys::refusing();
    let err = support
        .receive_message(&keys, VISITOR, draft("Where is my order? It is late."))
        .await
        .unwrap_err();
    assert!(matches!(err, SupportError::Other(_)));
    assert!(!err.is_user_facing());
    assert_eq!(keys.len(), 0);
}

#[tokio::test]
async fn what_a_visitor_writes_is_checked() {
    let support = command().await;
    let keys = MemoryKeys::default();
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
        let err = support
            .receive_message(&keys, VISITOR, cmd)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some(code));
        assert!(err.is_user_facing());
    }
    // A refused message draws no key.
    assert_eq!(keys.len(), 0);
    // An unreadable language is no language.
    let cmd = with(|c| c.language = Some("???".into()));
    let message = support.receive_message(&keys, VISITOR, cmd).await.unwrap();
    assert_eq!(message.language, None);
}

#[tokio::test]
async fn the_merchant_handles_and_reopens_a_message() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "Where is my order? It is late.").await;

    support.handle_message(ALICE, &message.id).await.unwrap();
    let handled = support.load_message(&message.id).await.unwrap().unwrap();
    assert_eq!(handled.status, MessageStatus::Handled);
    assert!(handled.is_handled());
    assert!(handled.handled_at.is_some());
    assert!(handled.updated_at >= message.updated_at);
    assert!(
        handled
            .open_with(&keys)
            .await
            .unwrap()
            .unwrap()
            .is_handled()
    );

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

#[tokio::test]
async fn the_merchant_erases_a_message() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "Please forget I ever wrote.").await;
    let kept = receive(&support, &keys, "Do you ship to Belgium?").await;
    let state = support.load_message(&message.id).await.unwrap().unwrap();

    support
        .forget_message(&keys, ALICE, &message.id)
        .await
        .unwrap();
    // The key is gone, the message with it; the other one stands.
    assert!(!keys.has(&state.key_id));
    assert_eq!(keys.len(), 1);
    assert_eq!(support.load_message(&message.id).await.unwrap(), None);
    assert!(support.load_message(&kept.id).await.unwrap().is_some());
    for result in [
        support.handle_message(ALICE, &message.id).await,
        support.reopen_message(ALICE, &message.id).await,
    ] {
        assert!(matches!(result, Err(SupportError::NotFound)));
    }

    // Again is a no-op; unknown is not found.
    support
        .forget_message(&keys, ALICE, &message.id)
        .await
        .unwrap();
    assert!(matches!(
        support.forget_message(&keys, ALICE, "nothing").await,
        Err(SupportError::NotFound)
    ));
}

#[tokio::test]
async fn a_message_without_its_key_does_not_open() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "Where is my order? It is late.").await;
    let state = support.load_message(&message.id).await.unwrap().unwrap();
    keys.lose_all();
    assert_eq!(state.open_with(&keys).await.unwrap(), None);
    // Another key does not open it either.
    let other = MemoryKeys::default();
    let foreign = receive(&support, &other, "Do you ship to Belgium?").await;
    let foreign_state = support.load_message(&foreign.id).await.unwrap().unwrap();
    let foreign_key = other.get(&foreign_state.key_id).await.unwrap().unwrap();
    assert!(state.open(&foreign_key).is_err());
}
