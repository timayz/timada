mod common;

use common::*;
use timada_proposals::{
    DEFAULT_VALID_SECS, FIELD_BODY, FIELD_DESCRIPTION, FIELD_NAME, KIND_CATEGORY, MAX_VALID_SECS,
    MAX_VALUE_LEN, Prepare, ProposalError, ProposalStatus,
};

#[tokio::test]
async fn a_prepared_proposal_waits_with_what_it_would_change() {
    let proposals = command().await;
    let id = prepared(&proposals).await;
    let proposal = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert_eq!(proposal.status, ProposalStatus::Waiting);
    assert_eq!(proposal.kind, "product");
    assert_eq!(proposal.subject_id, "p1");
    assert_eq!(proposal.subject_name, "Gants");
    assert_eq!(proposal.source, "writer");
    assert_eq!(proposal.prepared_by, BOB);
    assert_eq!(proposal.valid_until, NOW + DEFAULT_VALID_SECS);
    assert_eq!(
        proposal.changes,
        [
            change(FIELD_NAME, "Gants", "Gants moto été"),
            change(FIELD_DESCRIPTION, "", "Des gants légers."),
        ]
    );
    assert_eq!(proposals.load_proposal("nope").await.unwrap(), None);
}

#[tokio::test]
async fn what_is_not_a_proposal_is_refused() {
    let proposals = command().await;
    let refused = |cmd: Prepare| async {
        proposals
            .prepare(BOB, cmd, NOW)
            .await
            .unwrap_err()
            .code()
            .unwrap()
    };
    let with = |edit: fn(&mut Prepare)| {
        let mut cmd = texts();
        edit(&mut cmd);
        cmd
    };
    assert_eq!(
        refused(with(|cmd| cmd.kind = "order".into())).await,
        "proposal_unknown_kind"
    );
    assert_eq!(
        refused(with(|cmd| cmd.subject_id = "p 1".into())).await,
        "proposal_invalid_subject"
    );
    assert_eq!(
        refused(with(|cmd| cmd.source = " ".into())).await,
        "proposal_invalid_source"
    );
    assert_eq!(
        refused(with(|cmd| cmd.changes.clear())).await,
        "proposal_invalid_changes"
    );
    // A product has no body, and a field is changed once.
    assert_eq!(
        refused(with(|cmd| cmd.changes[0].field = FIELD_BODY.into())).await,
        "proposal_invalid_changes"
    );
    assert_eq!(
        refused(with(|cmd| cmd.changes[1].field = FIELD_NAME.into())).await,
        "proposal_invalid_changes"
    );
    assert_eq!(
        refused(with(
            |cmd| cmd.changes[0].after = "x".repeat(MAX_VALUE_LEN + 1)
        ))
        .await,
        "proposal_value_too_long"
    );
    assert_eq!(
        refused(with(|cmd| {
            cmd.changes = vec![change(FIELD_NAME, "Gants", "Gants")];
        }))
        .await,
        "proposal_nothing_to_change"
    );
    assert_eq!(
        refused(with(|cmd| cmd.reason = "x".repeat(1_001))).await,
        "proposal_reason_too_long"
    );
    assert_eq!(
        refused(with(|cmd| cmd.valid_until = Some(NOW))).await,
        "proposal_invalid_validity"
    );
    assert_eq!(
        refused(with(|cmd| cmd.valid_until = Some(NOW + MAX_VALID_SECS + 1))).await,
        "proposal_invalid_validity"
    );
}

/// A field that would take what it holds is left out of the proposal.
#[tokio::test]
async fn a_change_that_changes_nothing_is_left_out() {
    let proposals = command().await;
    let mut cmd = texts();
    cmd.kind = KIND_CATEGORY.to_owned();
    cmd.changes.push(change(FIELD_BODY, "Same", "Same"));
    let id = proposals.prepare(BOB, cmd, NOW).await.unwrap();
    let proposal = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert_eq!(proposal.changes.len(), 2);
    assert_eq!(proposal.change(FIELD_BODY), None);
}

#[tokio::test]
async fn an_approval_keeps_the_values_and_who_approved() {
    let proposals = command().await;
    let id = prepared(&proposals).await;
    proposals
        .approve(ALICE, &id, vec![], NOW + 60)
        .await
        .unwrap();
    let proposal = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert_eq!(proposal.status, ProposalStatus::Approved);
    assert_eq!(proposal.answered_by, ALICE);
    assert!(!proposal.edited);
    assert_eq!(proposal.changes, texts().changes);

    // Answered: neither approved again nor refused.
    let again = proposals.approve(ALICE, &id, vec![], NOW + 61).await;
    assert!(matches!(again, Err(ProposalError::NotWaiting)));
    let refused = proposals.refuse(ALICE, &id, "").await;
    assert!(matches!(refused, Err(ProposalError::NotWaiting)));

    assert!(proposals.record_applied("saga", &id).await.unwrap());
    // Another node saying the same writes nothing.
    assert!(!proposals.record_applied("saga", &id).await.unwrap());
    assert!(
        !proposals
            .record_apply_failure("saga", &id, "late")
            .await
            .unwrap()
    );
    let applied = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert_eq!(applied.status, ProposalStatus::Applied);
    assert_eq!(applied.answered_by, ALICE);
}

#[tokio::test]
async fn a_member_may_edit_before_approving() {
    let proposals = command().await;
    let id = prepared(&proposals).await;
    let unknown = proposals
        .approve(ALICE, &id, vec![("price".into(), "12".into())], NOW)
        .await;
    assert!(matches!(unknown, Err(ProposalError::UnknownField)));

    // The name reworded; the description set back to what the product
    // holds, so it is no longer part of the change.
    proposals
        .approve(
            ALICE,
            &id,
            vec![
                (FIELD_NAME.into(), "Gants moto d'été".into()),
                (FIELD_DESCRIPTION.into(), String::new()),
            ],
            NOW,
        )
        .await
        .unwrap();
    let proposal = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert!(proposal.edited);
    assert_eq!(
        proposal.changes,
        [change(FIELD_NAME, "Gants", "Gants moto d'été")]
    );

    // Every field set back: nothing left to approve.
    let other = prepared(&proposals).await;
    let nothing = proposals
        .approve(
            ALICE,
            &other,
            vec![
                (FIELD_NAME.into(), "Gants".into()),
                (FIELD_DESCRIPTION.into(), String::new()),
            ],
            NOW,
        )
        .await;
    assert!(matches!(nothing, Err(ProposalError::NothingToChange)));
}

#[tokio::test]
async fn a_proposal_past_its_date_is_refused_not_approved() {
    let proposals = command().await;
    let id = prepared(&proposals).await;
    let late = NOW + DEFAULT_VALID_SECS + 1;
    let proposal = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert!(!proposal.is_expired(late - 1));
    assert!(proposal.is_expired(late));
    let approved = proposals.approve(ALICE, &id, vec![], late).await;
    assert!(matches!(approved, Err(ProposalError::Expired)));

    proposals.refuse(ALICE, &id, "  Too late.  ").await.unwrap();
    let refused = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert_eq!(refused.status, ProposalStatus::Refused);
    assert_eq!(refused.answered_by, ALICE);
    assert_eq!(refused.note, "Too late.");
    assert!(!refused.is_expired(late));
}

#[tokio::test]
async fn a_changed_target_ends_the_proposal_out_of_date() {
    let proposals = command().await;
    // Found before anyone answered.
    let waiting = prepared(&proposals).await;
    assert!(
        proposals
            .record_out_of_date(ALICE, &waiting, FIELD_NAME, "Gants cuir")
            .await
            .unwrap()
    );
    let proposal = proposals.load_proposal(&waiting).await.unwrap().unwrap();
    assert_eq!(proposal.status, ProposalStatus::OutOfDate);
    assert_eq!(proposal.note, "name: Gants cuir");
    assert_eq!(proposal.answered_by, "");
    let approved = proposals.approve(ALICE, &waiting, vec![], NOW).await;
    assert!(matches!(approved, Err(ProposalError::NotWaiting)));

    // Found while applying what was approved.
    let approved = prepared(&proposals).await;
    proposals
        .approve(ALICE, &approved, vec![], NOW)
        .await
        .unwrap();
    assert!(
        proposals
            .record_out_of_date("saga", &approved, FIELD_NAME, "Gants cuir")
            .await
            .unwrap()
    );
    assert!(!proposals.record_applied("saga", &approved).await.unwrap());
    assert!(
        !proposals
            .record_out_of_date("saga", &approved, FIELD_NAME, "Autre")
            .await
            .unwrap()
    );
    let proposal = proposals.load_proposal(&approved).await.unwrap().unwrap();
    assert_eq!(proposal.status, ProposalStatus::OutOfDate);
    assert_eq!(proposal.answered_by, ALICE);
}

#[tokio::test]
async fn a_change_the_shop_refuses_ends_the_proposal_failed() {
    let proposals = command().await;
    let id = prepared(&proposals).await;
    // Nothing to fail before it is approved.
    assert!(
        !proposals
            .record_apply_failure("saga", &id, "early")
            .await
            .unwrap()
    );
    proposals.approve(ALICE, &id, vec![], NOW).await.unwrap();
    assert!(
        proposals
            .record_apply_failure("saga", &id, "The shop refused it: product archived.")
            .await
            .unwrap()
    );
    let proposal = proposals.load_proposal(&id).await.unwrap().unwrap();
    assert_eq!(proposal.status, ProposalStatus::Failed);
    assert_eq!(proposal.note, "The shop refused it: product archived.");
    let missing = proposals.record_applied("saga", "nope").await;
    assert!(matches!(missing, Err(ProposalError::NotFound)));
}
