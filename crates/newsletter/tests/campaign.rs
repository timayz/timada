mod common;

use std::sync::Arc;

use common::*;
use timada_newsletter::{
    CampaignDraft, CampaignStatus, NewsletterError, RequestSubscription, SOURCE_FORM, campaign_row,
    campaigns_of_tenant, count_audience, read_model_subscription, sending_campaigns,
    undelivered_subscribers,
};

const TENANT: &str = "tenant-acme";

fn draft(subject: &str) -> CampaignDraft {
    CampaignDraft {
        subject: format!("  {subject} "),
        html: "<p>New mugs are in.</p>".to_owned(),
        text: Some("   ".to_owned()),
        language: None,
    }
}

async fn catch_up(executor: &evento::Sqlite, pool: &sqlx::SqlitePool, keys: &Arc<MemoryKeys>) {
    read_model_subscription::<evento::Sqlite>(pool.clone(), TENANT, keys.clone())
        .run_once(executor)
        .await
        .unwrap();
}

/// Confirms `email`, in `language`.
async fn subscribe(newsletter: &Newsletter, keys: &MemoryKeys, email: &str, language: &str) {
    let outcome = newsletter
        .request_subscription(
            keys,
            VISITOR,
            RequestSubscription {
                email: email.to_owned(),
                language: Some(language.to_owned()),
                source: SOURCE_FORM.to_owned(),
            },
            now(),
        )
        .await
        .unwrap();
    let timada_newsletter::RequestOutcome::ToConfirm { token, .. } = outcome else {
        panic!("nothing to mail");
    };
    newsletter
        .confirm_subscription(keys, VISITOR, &token, now())
        .await
        .unwrap();
}

#[tokio::test]
async fn a_draft_is_edited_until_it_is_sent() {
    let newsletter = command().await;
    let id = newsletter
        .draft_campaign(ALICE, draft("October news"))
        .await
        .unwrap();
    let state = newsletter.load_campaign(&id).await.unwrap().unwrap();
    assert_eq!(state.subject, "October news");
    assert_eq!(state.text, None);
    assert_eq!(state.status, CampaignStatus::Draft);

    newsletter
        .edit_campaign(
            ALICE,
            &id,
            CampaignDraft {
                language: Some("fr-FR".to_owned()),
                text: Some("New mugs.".to_owned()),
                ..draft("Les nouveautés")
            },
        )
        .await
        .unwrap();
    let state = newsletter.load_campaign(&id).await.unwrap().unwrap();
    assert_eq!(state.subject, "Les nouveautés");
    assert_eq!(state.language.as_deref(), Some("fr"));
    assert_eq!(state.text.as_deref(), Some("New mugs."));

    newsletter.send_campaign(ALICE, &id).await.unwrap();
    let state = newsletter.load_campaign(&id).await.unwrap().unwrap();
    assert_eq!(state.status, CampaignStatus::Sending);
    assert!(state.sent_at.is_some());
    // Sent: no more edits, no second send.
    assert!(matches!(
        newsletter.edit_campaign(ALICE, &id, draft("Again")).await,
        Err(NewsletterError::CampaignSent)
    ));
    assert!(matches!(
        newsletter.send_campaign(ALICE, &id).await,
        Err(NewsletterError::CampaignSent)
    ));
    newsletter.finish_campaign("node", &id).await.unwrap();
    // Twice is a no-op.
    newsletter.finish_campaign("node", &id).await.unwrap();
    let state = newsletter.load_campaign(&id).await.unwrap().unwrap();
    assert_eq!(state.status, CampaignStatus::Sent);

    for bad in [
        draft(""),
        CampaignDraft {
            html: "  ".to_owned(),
            ..draft("Empty")
        },
        draft(&"x".repeat(201)),
    ] {
        assert!(bad.validate().is_err(), "{bad:?}");
    }
    assert!(matches!(
        newsletter.send_campaign(ALICE, "unknown").await,
        Err(NewsletterError::CampaignNotFound)
    ));
}

#[tokio::test]
async fn a_subscriber_is_mailed_a_campaign_once() {
    let newsletter = command().await;
    let id = newsletter
        .draft_campaign(ALICE, draft("News"))
        .await
        .unwrap();
    assert!(
        newsletter
            .claim_delivery("node-a", &id, "sub")
            .await
            .unwrap()
    );
    // Another node, or a second pass: refused.
    assert!(
        !newsletter
            .claim_delivery("node-b", &id, "sub")
            .await
            .unwrap()
    );
    // Another campaign is another mail.
    let other = newsletter
        .draft_campaign(ALICE, draft("More news"))
        .await
        .unwrap();
    assert!(
        newsletter
            .claim_delivery("node-b", &other, "sub")
            .await
            .unwrap()
    );

    newsletter
        .fail_delivery(&id, "sub", "550 mailbox unavailable")
        .await
        .unwrap();
    // Twice is a no-op.
    newsletter.fail_delivery(&id, "sub", "again").await.unwrap();
    assert!(matches!(
        newsletter.fail_delivery(&id, "never-claimed", "x").await,
        Err(NewsletterError::NotFound)
    ));
}

#[tokio::test]
async fn sending_walks_the_confirmed_subscribers_in_the_campaigns_language() {
    let (executor, pool) = executor_with_pool().await;
    let newsletter = timada_newsletter::Command(executor.clone());
    let keys = Arc::new(MemoryKeys::default());
    subscribe(&newsletter, &keys, "ada@example.com", "fr").await;
    subscribe(&newsletter, &keys, "grace@example.com", "fr").await;
    subscribe(&newsletter, &keys, "alan@example.com", "en").await;
    // Pending: not mailed news.
    request(&newsletter, &keys, "pending@example.com").await;

    let everyone = newsletter
        .draft_campaign(ALICE, draft("To all"))
        .await
        .unwrap();
    let french = newsletter
        .draft_campaign(
            ALICE,
            CampaignDraft {
                language: Some("fr".to_owned()),
                ..draft("En français")
            },
        )
        .await
        .unwrap();
    newsletter.send_campaign(ALICE, &french).await.unwrap();
    catch_up(&executor, &pool, &keys).await;

    assert_eq!(count_audience(&pool, TENANT, None).await.unwrap(), 3);
    assert_eq!(count_audience(&pool, TENANT, Some("fr")).await.unwrap(), 2);
    assert_eq!(
        sending_campaigns(&pool).await.unwrap(),
        vec![(TENANT.to_owned(), french.clone())]
    );

    // A batch of one, then the rest: what was delivered is not owed again.
    let first = undelivered_subscribers(&pool, TENANT, &french, Some("fr"), 1)
        .await
        .unwrap();
    assert_eq!(first.len(), 1);
    assert!(
        newsletter
            .claim_delivery("node", &french, &first[0].id)
            .await
            .unwrap()
    );
    catch_up(&executor, &pool, &keys).await;
    let rest = undelivered_subscribers(&pool, TENANT, &french, Some("fr"), 10)
        .await
        .unwrap();
    assert_eq!(rest.len(), 1);
    assert_ne!(rest[0].id, first[0].id);
    assert_eq!(rest[0].language.as_deref(), Some("fr"));
    assert!(
        newsletter
            .claim_delivery("node", &french, &rest[0].id)
            .await
            .unwrap()
    );
    newsletter
        .fail_delivery(&french, &rest[0].id, "refused")
        .await
        .unwrap();
    catch_up(&executor, &pool, &keys).await;
    assert!(
        undelivered_subscribers(&pool, TENANT, &french, Some("fr"), 10)
            .await
            .unwrap()
            .is_empty()
    );
    // The other campaign still owes everyone confirmed.
    assert_eq!(
        undelivered_subscribers(&pool, TENANT, &everyone, None, 10)
            .await
            .unwrap()
            .len(),
        3
    );

    newsletter.finish_campaign("node", &french).await.unwrap();
    catch_up(&executor, &pool, &keys).await;
    let row = campaign_row(&pool, TENANT, &french).await.unwrap().unwrap();
    assert_eq!(row.status, "sent");
    assert_eq!((row.delivered, row.failed), (1, 1));
    assert!(sending_campaigns(&pool).await.unwrap().is_empty());
    let rows = campaigns_of_tenant(&pool, TENANT, 10, 0).await.unwrap();
    assert_eq!(rows.len(), 2);
    let draft_row = rows.iter().find(|row| row.id == everyone).unwrap();
    assert_eq!(
        (draft_row.status.as_str(), draft_row.delivered),
        ("draft", 0)
    );
}
