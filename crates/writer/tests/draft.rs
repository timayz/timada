mod common;

use common::*;
use timada_writer::{DRAFT_STALE_SECS, DraftStatus, RequestDraft, WriterError};

#[tokio::test]
async fn a_shop_says_how_it_wants_to_be_written_for() {
    let writer = command().await;
    let brief = writer.load_brief().await.unwrap();
    assert_eq!((brief.tone.as_str(), brief.never.as_str()), ("", ""));
    // Nothing said is nothing written.
    writer.set_brief(ALICE, " ", "").await.unwrap();
    assert_eq!(writer.load_brief().await.unwrap().updated_at, 0);

    writer
        .set_brief(
            ALICE,
            " Direct, sans superlatifs. ",
            "Jamais « le meilleur ».",
        )
        .await
        .unwrap();
    let brief = writer.load_brief().await.unwrap();
    assert_eq!(brief.tone, "Direct, sans superlatifs.");
    assert_eq!(brief.never, "Jamais « le meilleur ».");
    let too_long = writer.set_brief(ALICE, &"x".repeat(1_001), "").await;
    assert!(matches!(too_long, Err(WriterError::BriefTooLong)));
    // Another shop's is its own.
    let other = timada_writer::Command::new((*writer).clone(), "other");
    assert_eq!(other.load_brief().await.unwrap().tone, "");
}

#[tokio::test]
async fn a_draft_is_asked_for_taken_on_once_and_recorded() {
    let writer = command().await;
    let id = writer.request_draft(ALICE, gloves()).await.unwrap();
    let draft = writer.load_draft(&id).await.unwrap().unwrap();
    assert_eq!(draft.status, DraftStatus::Requested);
    assert_eq!(draft.keyword, "gants moto été");
    assert_eq!(draft.requested_by, ALICE);

    // Nothing to record before a node took it on.
    assert!(
        !writer
            .record_draft("saga", &id, "proposal-1", 20_000, vec![])
            .await
            .unwrap()
    );
    assert!(writer.start_draft("node-a", &id).await.unwrap());
    assert!(!writer.start_draft("node-b", &id).await.unwrap());
    let started = writer.load_draft(&id).await.unwrap().unwrap();
    assert!(!started.is_lost(started.updated_at + DRAFT_STALE_SECS - 1));
    assert!(started.is_lost(started.updated_at + DRAFT_STALE_SECS));

    assert!(
        writer
            .record_draft(
                "saga",
                &id,
                "proposal-1",
                20_000,
                vec!["30 cm".to_owned(), "x".repeat(200)],
            )
            .await
            .unwrap()
    );
    let drafted = writer.load_draft(&id).await.unwrap().unwrap();
    assert_eq!(drafted.status, DraftStatus::Drafted);
    assert_eq!(drafted.proposal_id, "proposal-1");
    assert_eq!(drafted.cost_usd_micros, 20_000);
    assert_eq!(drafted.to_check[0], "30 cm");
    assert_eq!(drafted.to_check[1].chars().count(), 60);
    // Ended: neither failed nor recorded again.
    assert!(
        !writer
            .record_draft_failure("saga", &id, "late")
            .await
            .unwrap()
    );
    assert!(
        !writer
            .record_draft("saga", &id, "proposal-2", 1, vec![])
            .await
            .unwrap()
    );
    assert!(!writer.start_draft("node-a", "nope").await.unwrap());
}

#[tokio::test]
async fn a_draft_fails_with_why_before_or_after_it_is_taken_on() {
    let writer = command().await;
    let early = writer.request_draft(ALICE, gloves()).await.unwrap();
    assert!(
        writer
            .record_draft_failure("saga", &early, "The product is gone.")
            .await
            .unwrap()
    );
    assert!(!writer.start_draft("node-a", &early).await.unwrap());
    let late = writer.request_draft(ALICE, gloves()).await.unwrap();
    writer.start_draft("node-a", &late).await.unwrap();
    writer
        .record_draft_failure("saga", &late, "  The model did not answer. ")
        .await
        .unwrap();
    let failed = writer.load_draft(&late).await.unwrap().unwrap();
    assert_eq!(failed.status, DraftStatus::Failed);
    assert_eq!(failed.reason, "The model did not answer.");
}

#[tokio::test]
async fn what_is_not_a_draft_is_refused() {
    let writer = command().await;
    let with = |edit: fn(&mut RequestDraft)| {
        let mut cmd = gloves();
        edit(&mut cmd);
        cmd
    };
    let kind = writer
        .request_draft(ALICE, with(|cmd| cmd.kind = "order".into()))
        .await;
    assert!(matches!(kind, Err(WriterError::UnknownKind)));
    let subject = writer
        .request_draft(ALICE, with(|cmd| cmd.subject_id = " ".into()))
        .await;
    assert!(matches!(subject, Err(WriterError::InvalidSubject)));
    let keyword = writer
        .request_draft(ALICE, with(|cmd| cmd.keyword = "x".repeat(121)))
        .await;
    assert!(matches!(keyword, Err(WriterError::InvalidKeyword)));
}
