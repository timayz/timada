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
    let resolved = cmd.resolve_session(&hash, NOW).await.unwrap().unwrap();
    assert_eq!(resolved.user.id, user_id);
    assert_eq!(resolved.session.expires_at, NOW + 60);
    assert_eq!(resolved.session.user_agent.as_deref(), Some("tests"));
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

#[tokio::test]
async fn refresh_extends_a_live_session_only() {
    let cmd = command().await;
    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    let hash = [1u8; 32];
    cmd.start_session(&hash, &user_id, NOW + 60, context())
        .await
        .unwrap();

    assert!(cmd.refresh_session(&hash, NOW + 600).await.unwrap());
    let resolved = cmd
        .resolve_session(&hash, NOW + 120)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.session.expires_at, NOW + 600);
    assert!(resolved.session.refreshed_at.is_some());

    cmd.end_session(&hash).await.unwrap();
    assert!(
        !cmd.refresh_session(&hash, NOW + 900).await.unwrap(),
        "ended"
    );
    assert!(
        !cmd.refresh_session(&[2u8; 32], NOW + 900).await.unwrap(),
        "unknown"
    );
}

#[tokio::test]
async fn revoking_sessions_spares_the_asking_browser_and_later_sign_ins() {
    let cmd = command().await;
    let user_id = register_via_oauth2(&cmd, "google", "1").await;
    let laptop = [1u8; 32];
    let phone = [2u8; 32];
    cmd.start_session(&laptop, &user_id, NOW + 600, context())
        .await
        .unwrap();
    cmd.start_session(&phone, &user_id, NOW + 600, context())
        .await
        .unwrap();

    // The millisecond cutoff must be strictly after both starts.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    cmd.revoke_sessions(&user_id, &user_id, Some(&laptop))
        .await
        .unwrap();
    assert!(cmd.resolve_session(&laptop, NOW).await.unwrap().is_some());
    assert!(cmd.resolve_session(&phone, NOW).await.unwrap().is_none());

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let tablet = [3u8; 32];
    cmd.start_session(&tablet, &user_id, NOW + 600, context())
        .await
        .unwrap();
    assert!(
        cmd.resolve_session(&tablet, NOW).await.unwrap().is_some(),
        "started after the revocation"
    );

    // Everything, this browser included.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    cmd.revoke_sessions(&user_id, &user_id, None).await.unwrap();
    assert!(cmd.resolve_session(&laptop, NOW).await.unwrap().is_none());
    assert!(cmd.resolve_session(&tablet, NOW).await.unwrap().is_none());
}

#[tokio::test]
async fn only_the_user_or_an_admin_revokes_sessions() {
    let cmd = command().await;
    let admin = register_via_oauth2(&cmd, "google", "1").await;
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    let hash = [1u8; 32];
    cmd.start_session(&hash, &alice, NOW + 600, context())
        .await
        .unwrap();

    let err = cmd.revoke_sessions(&bob, &alice, None).await.unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    let err = cmd
        .revoke_sessions("nobody", &alice, None)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Forbidden), "{err}");
    assert!(cmd.resolve_session(&hash, NOW).await.unwrap().is_some());

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    cmd.revoke_sessions(&admin, &alice, None).await.unwrap();
    assert!(cmd.resolve_session(&hash, NOW).await.unwrap().is_none());

    let err = cmd
        .revoke_sessions(&admin, "ghost", None)
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UserNotFound), "{err}");
}
