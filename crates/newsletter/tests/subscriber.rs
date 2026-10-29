mod common;

use common::*;
use timada_newsletter::{
    CONFIRM_TTL_SECS, ConfirmOutcome, NewsletterError, RESEND_AFTER_SECS, RequestOutcome,
    RequestSubscription, SubscriberStatus, subscriber_id,
};

#[tokio::test]
async fn a_request_counts_once_its_link_is_followed() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let outcome = newsletter
        .request_subscription(&keys, VISITOR, ask("  Ada@Example.com "), now())
        .await
        .unwrap();
    let RequestOutcome::ToConfirm {
        subscriber_id: id,
        email,
        language,
        token,
    } = outcome
    else {
        panic!("nothing to mail");
    };
    assert_eq!(id, subscriber_id(EMAIL));
    assert_eq!(email, EMAIL);
    assert_eq!(language.as_deref(), Some("fr"));

    let state = newsletter.load_subscriber(&id).await.unwrap().unwrap();
    assert_eq!(state.status, SubscriberStatus::Pending);
    assert!(!state.is_confirmed());
    // The address is sealed in the store, and opens with the key.
    assert!(!String::from_utf8_lossy(&state.email).contains("ada"));
    let opened = state.open_with(&keys).await.unwrap().unwrap();
    assert_eq!(opened.email, EMAIL);
    assert_eq!(opened.source, "form");

    assert_eq!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &token, now())
            .await
            .unwrap(),
        ConfirmOutcome::Confirmed
    );
    // Followed twice: nothing changes.
    assert_eq!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &token, now())
            .await
            .unwrap(),
        ConfirmOutcome::AlreadyConfirmed
    );
    let state = newsletter
        .subscriber_of("ADA@example.com")
        .await
        .unwrap()
        .unwrap();
    assert!(state.is_confirmed());
    assert!(state.confirmed_at.is_some());

    // On the list already: nothing more to mail.
    assert_eq!(
        newsletter
            .request_subscription(&keys, VISITOR, ask(EMAIL), now())
            .await
            .unwrap(),
        RequestOutcome::AlreadyConfirmed
    );
}

#[tokio::test]
async fn asking_again_soon_mails_nothing() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let (_, first) = request(&newsletter, &keys, EMAIL).await;
    assert_eq!(
        newsletter
            .request_subscription(&keys, VISITOR, ask(EMAIL), now())
            .await
            .unwrap(),
        RequestOutcome::RecentlyRequested
    );
    // Later, a new link goes out under the same key; the old one is stale.
    let later = now() + RESEND_AFTER_SECS + 1;
    let RequestOutcome::ToConfirm { token: second, .. } = newsletter
        .request_subscription(&keys, VISITOR, ask(EMAIL), later)
        .await
        .unwrap()
    else {
        panic!("nothing to mail");
    };
    assert_eq!(keys.len(), 1);
    // Both were written in the same second here, so they may be the same
    // link; when they are not, only the newest confirms.
    if first != second {
        assert!(matches!(
            newsletter
                .confirm_subscription(&keys, VISITOR, &first, now())
                .await,
            Err(NewsletterError::InvalidLink)
        ));
    }
    newsletter
        .confirm_subscription(&keys, VISITOR, &second, now())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_link_that_does_not_check_out_is_refused() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let (id, token) = request(&newsletter, &keys, EMAIL).await;
    let tampered = format!("{}x", token);
    for bad in ["", "nonsense", tampered.as_str(), &format!("{id}.1.abc")] {
        assert!(
            matches!(
                newsletter
                    .confirm_subscription(&keys, VISITOR, bad, now())
                    .await,
                Err(NewsletterError::InvalidLink)
            ),
            "{bad:?}"
        );
    }
    // Too late.
    assert!(matches!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &token, now() + CONFIRM_TTL_SECS + 5)
            .await,
        Err(NewsletterError::LinkExpired)
    ));
    // Another address's link does not confirm this one.
    let (_, other) = request(&newsletter, &keys, "grace@example.com").await;
    let swapped = other.replacen(&subscriber_id("grace@example.com"), &id, 1);
    assert!(matches!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &swapped, now())
            .await,
        Err(NewsletterError::InvalidLink)
    ));
}

#[tokio::test]
async fn bad_requests_are_refused() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let bad_email = RequestSubscription {
        email: "nope".to_owned(),
        ..ask(EMAIL)
    };
    assert!(matches!(
        bad_email.validate(),
        Err(NewsletterError::InvalidEmail)
    ));
    let bad_source = RequestSubscription {
        source: "elsewhere".to_owned(),
        ..ask(EMAIL)
    };
    assert!(matches!(
        newsletter
            .request_subscription(&keys, VISITOR, bad_source, now())
            .await,
        Err(NewsletterError::InvalidSource)
    ));
    // No key held, nothing recorded.
    let refusing = MemoryKeys::refusing();
    assert!(
        newsletter
            .request_subscription(&refusing, VISITOR, ask(EMAIL), now())
            .await
            .is_err()
    );
    assert!(newsletter.subscriber_of(EMAIL).await.unwrap().is_none());
}

#[tokio::test]
async fn a_mail_link_takes_them_off_the_list() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let (id, token) = request(&newsletter, &keys, EMAIL).await;
    newsletter
        .confirm_subscription(&keys, VISITOR, &token, now())
        .await
        .unwrap();
    let state = newsletter.load_subscriber(&id).await.unwrap().unwrap();
    let link = state.unsubscribe_token(&keys).await.unwrap().unwrap();
    // A confirmation link is not an unsubscribe link, and the other way round.
    assert!(matches!(
        newsletter
            .unsubscribe_with_link(&keys, VISITOR, &token)
            .await,
        Err(NewsletterError::InvalidLink)
    ));
    assert!(matches!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &link, now())
            .await,
        Err(NewsletterError::InvalidLink)
    ));

    assert_eq!(
        newsletter
            .unsubscribe_with_link(&keys, VISITOR, &link)
            .await
            .unwrap(),
        id
    );
    // Twice is fine.
    newsletter
        .unsubscribe_with_link(&keys, VISITOR, &link)
        .await
        .unwrap();
    let state = newsletter.load_subscriber(&id).await.unwrap().unwrap();
    assert_eq!(state.status, SubscriberStatus::Unsubscribed);
    // The old confirmation link does not put them back.
    assert!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &token, now())
            .await
            .is_err()
    );
    // Asking again does, after a new link.
    let RequestOutcome::ToConfirm { token, .. } = newsletter
        .request_subscription(&keys, VISITOR, ask(EMAIL), now())
        .await
        .unwrap()
    else {
        panic!("nothing to mail");
    };
    newsletter
        .confirm_subscription(&keys, VISITOR, &token, now())
        .await
        .unwrap();
    // The unsubscribe link of the earlier mails still works: same key.
    newsletter
        .unsubscribe_with_link(&keys, VISITOR, &link)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_signed_in_shopper_is_confirmed_at_once() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    newsletter
        .subscribe_verified(&keys, VISITOR, EMAIL, Some("en".to_owned()), None)
        .await
        .unwrap();
    let state = newsletter.subscriber_of(EMAIL).await.unwrap().unwrap();
    assert!(state.is_confirmed());
    assert_eq!(state.source, "account");
    // Twice is a no-op.
    newsletter
        .subscribe_verified(&keys, VISITOR, EMAIL, None, None)
        .await
        .unwrap();
    newsletter.unsubscribe(VISITOR, &state.id).await.unwrap();
    newsletter
        .subscribe_verified(&keys, VISITOR, EMAIL, None, None)
        .await
        .unwrap();
    assert!(
        newsletter
            .subscriber_of(EMAIL)
            .await
            .unwrap()
            .unwrap()
            .is_confirmed()
    );
    assert!(matches!(
        newsletter.unsubscribe(VISITOR, "unknown").await,
        Err(NewsletterError::NotFound)
    ));
}

#[tokio::test]
async fn the_browsers_time_zone_follows_the_subscriber() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let asked = RequestSubscription {
        time_zone: Some("europe/paris".to_owned()),
        ..ask(EMAIL)
    };
    let RequestOutcome::ToConfirm { token, .. } = newsletter
        .request_subscription(&keys, VISITOR, asked, now())
        .await
        .unwrap()
    else {
        panic!("nothing to mail");
    };
    let state = newsletter.subscriber_of(EMAIL).await.unwrap().unwrap();
    assert_eq!(state.time_zone.as_deref(), Some("Europe/Paris"));
    newsletter
        .confirm_subscription(&keys, VISITOR, &token, now())
        .await
        .unwrap();

    // Garbage is ignored and keeps the zone known.
    let garbage = RequestSubscription {
        time_zone: Some("Mars/Olympus".to_owned()),
        ..ask(EMAIL)
    };
    newsletter
        .request_subscription(&keys, VISITOR, garbage, now())
        .await
        .unwrap();
    // On the list already, asked again from elsewhere: the zone moves.
    newsletter
        .subscribe_verified(
            &keys,
            VISITOR,
            EMAIL,
            None,
            Some("America/New_York".to_owned()),
        )
        .await
        .unwrap();
    let state = newsletter.subscriber_of(EMAIL).await.unwrap().unwrap();
    assert_eq!(state.time_zone.as_deref(), Some("America/New_York"));
    assert!(state.is_confirmed());
    let opened = state.open_with(&keys).await.unwrap().unwrap();
    assert_eq!(opened.time_zone.as_deref(), Some("America/New_York"));
}

#[tokio::test]
async fn erasing_kills_the_address_and_its_links() {
    let newsletter = command().await;
    let keys = MemoryKeys::default();
    let (id, token) = request(&newsletter, &keys, EMAIL).await;
    let state = newsletter.load_subscriber(&id).await.unwrap().unwrap();
    let link = state.unsubscribe_token(&keys).await.unwrap().unwrap();
    newsletter
        .forget_subscriber(&keys, ALICE, &id)
        .await
        .unwrap();
    // Twice is a no-op.
    newsletter
        .forget_subscriber(&keys, ALICE, &id)
        .await
        .unwrap();
    assert!(!keys.has(&state.key_id));
    assert!(newsletter.load_subscriber(&id).await.unwrap().is_none());
    assert!(
        newsletter
            .confirm_subscription(&keys, VISITOR, &token, now())
            .await
            .is_err()
    );
    assert!(
        newsletter
            .unsubscribe_with_link(&keys, VISITOR, &link)
            .await
            .is_err()
    );
    assert!(matches!(
        newsletter.forget_subscriber(&keys, ALICE, "unknown").await,
        Err(NewsletterError::NotFound)
    ));
    // The address may subscribe again, under a fresh key.
    let (again, _) = request(&newsletter, &keys, EMAIL).await;
    assert_eq!(again, id);
    let fresh = newsletter.load_subscriber(&id).await.unwrap().unwrap();
    assert_ne!(fresh.key_id, state.key_id);
    assert_eq!(fresh.status, SubscriberStatus::Pending);
}
