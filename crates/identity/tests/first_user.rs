mod common;

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use common::*;
use timada_identity::{OAuth2Mode, REGISTRY_ID};

#[tokio::test]
async fn exactly_one_of_many_concurrent_first_registrations_becomes_admin() {
    let cmd = command().await;
    let mut registrations: Vec<Pin<Box<dyn Future<Output = String> + '_>>> = Vec::new();
    for i in 0..10 {
        let cmd = &cmd;
        registrations.push(Box::pin(async move {
            cmd.complete_oauth2(oauth2(
                OAuth2Mode::CreateUser,
                None,
                "google",
                &format!("u{i}"),
            ))
            .await
            .unwrap()
            .user_id
        }));
    }
    let ids = join_all(registrations).await;
    let mut admins = 0;
    for id in &ids {
        if cmd.load_user(id).await.unwrap().unwrap().is_admin {
            admins += 1;
        }
    }
    assert_eq!(admins, 1);
    let registry = cmd.load_registry().await.unwrap().unwrap();
    assert!(ids.contains(&registry.first_user_id));
}

#[tokio::test]
async fn a_missing_admin_grant_is_repaired_on_sign_in() {
    let cmd = command().await;
    // Simulate a crash between the claim and the grant: the claim exists for a
    // user who was never granted admin.
    let user_id = {
        // Claim the registry for an id first, then register that user by hand
        // through a passkey flow using the same bound id.
        let id = new_id();
        evento::append(REGISTRY_ID)
            .original_version(0)
            .event(&timada_identity::aggregator::FirstUserClaimed {
                user_id: id.clone(),
            })
            .commit(&cmd.0)
            .await
            .unwrap();
        id
    };
    cmd.register_passkey(register_passkey(
        timada_identity::RegistrationMode::CreateUser,
        None,
        &user_id,
        "cred-1",
    ))
    .await
    .unwrap();
    // Registration lost the claim (someone else holds it), so no grant yet…
    // except the claim names this very user, and the sign-in notices.
    let user = cmd.load_user(&user_id).await.unwrap().unwrap();
    assert!(
        user.is_admin,
        "sign-in during registration repaired the grant"
    );

    let other = register_via_oauth2(&cmd, "google", "2").await;
    assert!(!cmd.load_user(&other).await.unwrap().unwrap().is_admin);
}

/// Polls every future concurrently and collects the results in order.
async fn join_all<'a, T>(mut futures: Vec<Pin<Box<dyn Future<Output = T> + 'a>>>) -> Vec<T> {
    let mut results: Vec<Option<T>> = (0..futures.len()).map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (i, fut) in futures.iter_mut().enumerate() {
            if results[i].is_some() {
                continue;
            }
            match fut.as_mut().poll(cx) {
                Poll::Ready(v) => results[i] = Some(v),
                Poll::Pending => pending = true,
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    results.into_iter().map(|r| r.unwrap()).collect()
}
