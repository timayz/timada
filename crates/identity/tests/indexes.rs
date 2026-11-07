//! An index entry and its user are two streams: what a crash between the two
//! writes leaves behind, and how it settles.

mod common;

use common::*;
use evento::WriteError;
use timada_identity::aggregator::{
    EmailAccount, OAuth2Account, OAuth2AccountLinked, PasskeyAdded, PasskeyCredential, Released,
    Removed, Unlinked,
};
use timada_identity::{
    Command, IdentityError, OAuth2Mode, count_users, email_account_id, oauth2_account_id,
    passkey_credential_id, read_model_subscription,
};

/// The names of a stream's events, oldest first.
async fn events_of<A: evento::Aggregate>(store: &evento::Turso, id: String) -> Vec<String> {
    evento::read::<A>(id)
        .execute(store)
        .await
        .unwrap()
        .into_iter()
        .map(|event| event.name)
        .collect()
}

async fn passkey_entry(store: &evento::Turso, credential_id: &str) -> Vec<String> {
    events_of::<PasskeyCredential>(store, passkey_credential_id(credential_id)).await
}

async fn oauth2_entry(store: &evento::Turso, provider: &str, sub: &str) -> Vec<String> {
    events_of::<OAuth2Account>(store, oauth2_account_id(provider, sub)).await
}

async fn email_entry(store: &evento::Turso, email: &str) -> Vec<String> {
    events_of::<EmailAccount>(store, email_account_id(email)).await
}

#[tokio::test]
async fn a_removed_passkey_signs_nobody_in_until_its_entry_is_freed() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();

    // The node stops before the entry is freed: the account says removed.
    refusing_command(Refusing::new(store.clone()).refuse::<Removed>())
        .remove_passkey(&alice, "cred-1")
        .await
        .unwrap();
    assert!(
        cmd.load_user(&alice)
            .await
            .unwrap()
            .unwrap()
            .passkeys
            .is_empty()
    );
    assert_eq!(passkey_entry(&store, "cred-1").await, ["Created"]);
    assert!(cmd.load_passkey("cred-1").await.unwrap().is_none());
    let err = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");

    // The subscription frees it.
    release_indexes(&store).await;
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Removed"]
    );
}

#[tokio::test]
async fn the_subscription_frees_nothing_twice_nor_what_was_added_back() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-2"))
        .await
        .unwrap();

    // Freed at once by the command: told again, nothing happens.
    cmd.remove_passkey(&alice, "cred-1").await.unwrap();
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Removed"]
    );
    // Removed, then added back before the subscription came by.
    cmd.remove_passkey(&alice, "cred-2").await.unwrap();
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-2"))
        .await
        .unwrap();

    release_indexes(&store).await;
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Removed"]
    );
    assert_eq!(
        passkey_entry(&store, "cred-2").await,
        ["Created", "Removed", "Created"]
    );
    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-2", None, None))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
}

#[tokio::test]
async fn an_entry_left_behind_is_held_until_released() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    refusing_command(Refusing::new(store.clone()).refuse::<Removed>())
        .remove_passkey(&alice, "cred-1")
        .await
        .unwrap();

    // Claimed moments ago, it is held for the grace: it signs nobody in,
    // and nobody else takes it until it is released.
    let err = cmd
        .add_passkey(add_passkey(Some(&bob), &bob, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");

    release_indexes(&store).await;
    cmd.add_passkey(add_passkey(Some(&bob), &bob, "cred-1"))
        .await
        .unwrap();
    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, bob);
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Removed", "Created"]
    );
}

#[tokio::test]
async fn an_unlinked_identity_signs_nobody_in_and_registers_anew() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.complete_oauth2(oauth2(
        OAuth2Mode::AddToUser,
        Some(&alice),
        "auth0",
        "alice",
    ))
    .await
    .unwrap();

    refusing_command(Refusing::new(store.clone()).refuse::<Unlinked>())
        .unlink_oauth2(&alice, "auth0", "alice")
        .await
        .unwrap();
    assert_eq!(oauth2_entry(&store, "auth0", "alice").await, ["Linked"]);
    assert!(
        cmd.load_oauth2_account("auth0", "alice")
            .await
            .unwrap()
            .is_none()
    );
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "auth0", "alice"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");

    // Released, a new account gets the identity.
    release_indexes(&store).await;
    assert_eq!(
        oauth2_entry(&store, "auth0", "alice").await,
        ["Linked", "Unlinked"]
    );
    let outcome = cmd
        .complete_oauth2(oauth2(
            OAuth2Mode::CreateUserOrLogin,
            None,
            "auth0",
            "alice",
        ))
        .await
        .unwrap();
    assert!(outcome.created);
    assert_ne!(outcome.user_id, alice);
    assert_eq!(
        cmd.load_oauth2_account("auth0", "alice")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        outcome.user_id
    );
    assert_eq!(
        cmd.load_user(&alice)
            .await
            .unwrap()
            .unwrap()
            .sign_in_methods(),
        1
    );
}

#[tokio::test]
async fn a_refused_add_gives_the_passkey_back() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;

    let err = refusing_command(Refusing::new(store.clone()).lose_race::<PasskeyAdded>())
        .add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            IdentityError::Write(WriteError::InvalidOriginalVersion)
        ),
        "{err}"
    );
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Removed"]
    );
    assert!(
        cmd.load_user(&alice)
            .await
            .unwrap()
            .unwrap()
            .passkeys
            .is_empty()
    );

    // Trying again works.
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    assert_eq!(
        cmd.load_passkey("cred-1").await.unwrap().unwrap().user_id,
        alice
    );
}

#[tokio::test]
async fn an_add_whose_outcome_is_unknown_keeps_its_claim() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    // The account write may still land: the claim stays, held for the grace.
    let err = refusing_command(Refusing::new(store.clone()).refuse::<PasskeyAdded>())
        .add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Write(_)), "{err}");
    assert_eq!(passkey_entry(&store, "cred-1").await, ["Created"]);
    let err = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");
    let err = cmd
        .add_passkey(add_passkey(Some(&bob), &bob, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");

    // Alice trying again finishes it.
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    let outcome = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
}

#[tokio::test]
async fn a_refused_link_gives_the_identity_back() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;

    let err = refusing_command(Refusing::new(store.clone()).lose_race::<OAuth2AccountLinked>())
        .complete_oauth2(oauth2(
            OAuth2Mode::AddToUser,
            Some(&alice),
            "auth0",
            "alice",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::InvalidState), "{err}");
    assert_eq!(
        oauth2_entry(&store, "auth0", "alice").await,
        ["Linked", "Unlinked"]
    );
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "auth0", "alice"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");
}

#[tokio::test]
async fn a_link_on_its_way_keeps_the_identity_from_another_account() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;

    // Alice links her Auth0 identity; the account write may still land.
    let err = refusing_command(Refusing::new(store.clone()).refuse::<OAuth2AccountLinked>())
        .complete_oauth2(oauth2(
            OAuth2Mode::AddToUser,
            Some(&alice),
            "auth0",
            "alice",
        ))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::Write(_)), "{err}");
    assert_eq!(oauth2_entry(&store, "auth0", "alice").await, ["Linked"]);

    // Meanwhile the identity takes no other account, nor signs anyone in.
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::AddToUser, Some(&bob), "auth0", "alice"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, IdentityError::AlreadyLinkedToOtherUser),
        "{err}"
    );
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "auth0", "alice"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");

    // Alice trying again finishes the link.
    cmd.complete_oauth2(oauth2(
        OAuth2Mode::AddToUser,
        Some(&alice),
        "auth0",
        "alice",
    ))
    .await
    .unwrap();
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "auth0", "alice"))
        .await
        .unwrap();
    assert_eq!(outcome.user_id, alice);
}

#[tokio::test]
async fn a_deletion_that_stopped_halfway_is_finished_by_the_subscription() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    cmd.add_passkey(add_passkey(Some(&bob), &bob, "cred-bob"))
        .await
        .unwrap();
    let carol = register_via_email(&cmd, "carol@example.com").await;

    let stops = Refusing::new(store.clone())
        .refuse::<Unlinked>()
        .refuse::<Removed>()
        .refuse::<Released>();
    let stopping = refusing_command(stops);
    stopping.delete_user(&root, &bob).await.unwrap();
    stopping.delete_user(&root, &carol).await.unwrap();
    assert_eq!(oauth2_entry(&store, "google", "bob").await, ["Linked"]);
    assert_eq!(passkey_entry(&store, "cred-bob").await, ["Created"]);
    assert_eq!(email_entry(&store, "carol@example.com").await, ["Claimed"]);

    // A deleted account signs in through none of them meanwhile.
    let err = cmd
        .complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "bob"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::NotRegistered), "{err}");
    let err = cmd
        .sign_in_passkey(sign_in_passkey("cred-bob", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");
    assert!(
        cmd.load_email_account("carol@example.com")
            .await
            .unwrap()
            .is_none()
    );

    release_indexes(&store).await;
    assert_eq!(
        oauth2_entry(&store, "google", "bob").await,
        ["Linked", "Unlinked"]
    );
    assert_eq!(
        passkey_entry(&store, "cred-bob").await,
        ["Created", "Removed"]
    );
    assert_eq!(
        email_entry(&store, "carol@example.com").await,
        ["Claimed", "Released"]
    );
}

#[tokio::test]
async fn registering_again_after_a_halfway_deletion_leaves_no_stub() {
    let (store, pool) = executor_with_pool().await;
    let cmd = Command::new(store.clone(), codes());
    let root = register_via_oauth2(&cmd, "google", "root").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    let carol = register_via_email(&cmd, "carol@example.com").await;
    let stopping = refusing_command(
        Refusing::new(store.clone())
            .refuse::<Unlinked>()
            .refuse::<Released>(),
    );
    stopping.delete_user(&root, &bob).await.unwrap();
    stopping.delete_user(&root, &carol).await.unwrap();

    // Before the subscription ran: the entries of the deleted accounts are
    // taken over, not raced into an unreachable stub.
    let outcome = cmd
        .complete_oauth2(oauth2(OAuth2Mode::CreateUserOrLogin, None, "google", "bob"))
        .await
        .unwrap();
    assert!(outcome.created);
    let again = register_via_email(&cmd, "carol@example.com").await;
    assert_eq!(
        cmd.load_oauth2_account("google", "bob")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        outcome.user_id
    );
    assert_eq!(
        cmd.load_email_account("carol@example.com")
            .await
            .unwrap()
            .unwrap()
            .user_id,
        again
    );

    // The late release leaves the new owners alone.
    release_indexes(&store).await;
    assert_eq!(
        oauth2_entry(&store, "google", "bob").await,
        ["Linked", "Linked"]
    );
    assert_eq!(
        email_entry(&store, "carol@example.com").await,
        ["Claimed", "Claimed"]
    );
    read_model_subscription::<evento::Turso>(pool.clone())
        .run_once(&store)
        .await
        .unwrap();
    assert_eq!(
        count_users(&pool, None).await.unwrap(),
        5,
        "root, the two deleted accounts and their two new ones"
    );
}

#[tokio::test]
async fn the_subscription_leaves_an_entry_claimed_since_the_removal() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    let bob = register_via_oauth2(&cmd, "google", "bob").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    refusing_command(Refusing::new(store.clone()).refuse::<Removed>())
        .remove_passkey(&alice, "cred-1")
        .await
        .unwrap();

    // Alice adds it again: the entry is claimed afresh, and her account
    // write may still land (an add on its way, as a late node sees it).
    refusing_command(Refusing::new(store.clone()).refuse::<PasskeyAdded>())
        .add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap_err();
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Created"]
    );

    // The release of the old removal is not this claim's.
    release_indexes(&store).await;
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Created"]
    );

    // Listed nowhere, it signs nobody in, and is held for the grace.
    let err = cmd
        .sign_in_passkey(sign_in_passkey("cred-1", None, None))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::UnknownCredential), "{err}");
    let err = cmd
        .add_passkey(add_passkey(Some(&bob), &bob, "cred-1"))
        .await
        .unwrap_err();
    assert!(matches!(err, IdentityError::AlreadyRegistered), "{err}");
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();
    assert_eq!(
        cmd.load_passkey("cred-1").await.unwrap().unwrap().user_id,
        alice
    );
}

#[tokio::test]
async fn the_free_after_a_removal_leaves_a_claim_made_since() {
    let store = executor().await;
    let cmd = Command::new(store.clone(), codes());
    let alice = register_via_oauth2(&cmd, "google", "alice").await;
    cmd.add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap();

    // The removal reads the entry to free only once Alice, in another tab,
    // has claimed it again for an add whose account write is on its way.
    let staged = Refusing::new(store.clone())
        .hold_first_read::<PasskeyCredential>()
        .refuse::<PasskeyAdded>();
    let removing = {
        let cmd = refusing_command(staged.clone());
        let alice = alice.clone();
        tokio::spawn(async move { cmd.remove_passkey(&alice, "cred-1").await })
    };
    staged.parked().await;
    refusing_command(staged)
        .add_passkey(add_passkey(Some(&alice), &alice, "cred-1"))
        .await
        .unwrap_err();
    removing.await.unwrap().unwrap();

    // The claim made since the removal is not the removal's to free.
    assert_eq!(
        passkey_entry(&store, "cred-1").await,
        ["Created", "Created"]
    );
}
