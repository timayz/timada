mod common;

use common::*;
use timada_identity::IdentityError;

const NOW: u64 = 1_700_000_000;

#[tokio::test]
async fn start_resolve_end() {
    let cmd = command().await;
    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    let hash = [1u8; 32];

    assert!(cmd.resolve_session(&hash, NOW).await.unwrap().is_none());

    cmd.start_session(&hash, &user_id, NOW + 60, context())
        .await
        .unwrap();
    let user = cmd.resolve_session(&hash, NOW).await.unwrap().unwrap();
    assert_eq!(user.id, user_id);
    assert!(
        cmd.resolve_session(&hash, NOW + 60)
            .await
            .unwrap()
            .is_none(),
        "expired"
    );
    assert!(
        cmd.resolve_session(&[2u8; 32], NOW)
            .await
            .unwrap()
            .is_none(),
        "unknown"
    );

    assert_eq!(cmd.end_session(&hash).await.unwrap(), Some(user_id.clone()));
    assert!(cmd.resolve_session(&hash, NOW).await.unwrap().is_none());
    assert_eq!(cmd.end_session(&hash).await.unwrap(), None, "already ended");
    assert_eq!(cmd.end_session(&[9u8; 32]).await.unwrap(), None, "unknown");
}

#[tokio::test]
async fn a_token_hash_cannot_be_reused_or_point_at_a_ghost() {
    let cmd = command().await;
    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    let hash = [1u8; 32];
    cmd.start_session(&hash, &user_id, NOW + 60, context())
        .await
        .unwrap();
    let err = cmd
        .start_session(&hash, &user_id, NOW + 60, context())
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidState), "{err}");

    let err = cmd
        .start_session(&[3u8; 32], "nobody", NOW + 60, context())
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UserNotFound), "{err}");
}
