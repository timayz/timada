mod common;

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use common::*;
use timada_identity::aggregator::{FirstUserClaimed, Linked, OAuth2AccountLinked, Registered};
use timada_identity::{OAuth2Mode, REGISTRY_ID, SignInMethod, oauth2_account_id};

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
    // Simulate a crash between the claim and the grant: the user and their
    // provider identity exist and the registry names them, but no grant
    // landed. Written by hand, exactly as `register_oauth2` would have.
    let user_id = new_id();
    evento::append(REGISTRY_ID)
        .original_version(0)
        .event(&FirstUserClaimed {
            user_id: user_id.clone(),
        })
        .commit(&cmd.0)
        .await
        .unwrap();
    evento::append(&user_id)
        .original_version(0)
        .event(&Registered {
            account: "1@example.com".to_owned(),
            label: "User 1".to_owned(),
            method: SignInMethod::OAuth2,
        })
        .event(&OAuth2AccountLinked {
            provider: "google".to_owned(),
            provider_user_id: "1".to_owned(),
            name: None,
            email: None,
            picture: None,
        })
        .commit(&cmd.0)
        .await
        .unwrap();
    evento::append(oauth2_account_id("google", "1"))
        .original_version(0)
        .event(&Linked {
            user_id: user_id.clone(),
            provider: "google".to_owned(),
            provider_user_id: "1".to_owned(),
        })
        .commit(&cmd.0)
        .await
        .unwrap();
    assert!(!cmd.load_user(&user_id).await.unwrap().unwrap().is_admin);

    // The next sign-in notices that the claim names this very user.
    cmd.complete_oauth2(oauth2(OAuth2Mode::Login, None, "google", "1"))
        .await
        .unwrap();
    assert!(cmd.load_user(&user_id).await.unwrap().unwrap().is_admin);

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
