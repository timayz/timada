mod common;

use common::*;

#[tokio::test]
async fn a_message_is_told_once() {
    let support = command().await;
    let keys = MemoryKeys::default();
    let message = receive(&support, &keys, "Where is my order? It is late.").await;
    let other = receive(&support, &keys, "Do you ship to Belgium?").await;
    assert_eq!(
        support
            .load_message_notification(&message.id)
            .await
            .unwrap(),
        None
    );

    assert!(
        support
            .claim_notification(&message.id, "node-a")
            .await
            .unwrap()
    );
    // Another node, or this one again: told already.
    assert!(
        !support
            .claim_notification(&message.id, "node-b")
            .await
            .unwrap()
    );
    let notice = support
        .load_message_notification(&message.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(notice.message_id, message.id);
    assert_eq!(notice.mailed_by, "node-a");
    assert!(notice.mailed_at > 0);

    // Each message has its own notice.
    assert!(
        support
            .claim_notification(&other.id, "node-b")
            .await
            .unwrap()
    );
}
