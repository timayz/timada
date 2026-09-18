use timada_review::{
    AnswerAuthor, AskQuestion, Command, ReviewError, ReviewStatus, SubmitReview,
    load_review_details, migrations, product_rating, product_summary_subscription,
};

const PRODUCT: &str = "aoc-24g4xe";

fn review(customer_id: &str, rating: u8) -> SubmitReview {
    SubmitReview {
        product_id: PRODUCT.into(),
        customer_id: customer_id.into(),
        order_id: Some("4112117449224J".into()),
        rating,
        title: "Construisez vos victoires".into(),
        body: "180 Hz, 1 ms, rien à redire.".into(),
    }
}

#[tokio::test]
async fn published_reviews_feed_the_product_rating() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(executor.clone());

    let too_high = cmd.submit_review(review("jonathan", 6)).await;
    assert!(matches!(too_high, Err(ReviewError::InvalidRating(6))));

    let first = cmd.submit_review(review("jonathan", 4)).await?;
    cmd.publish_review(&first).await?;
    let duplicate = cmd.submit_review(review("jonathan", 5)).await;
    assert!(matches!(duplicate, Err(ReviewError::AlreadyReviewed)));

    let second = cmd.submit_review(review("marie", 5)).await?;
    cmd.publish_review(&second).await?;

    let view = load_review_details(&executor, &first)
        .await?
        .ok_or_else(|| anyhow::anyhow!("review view missing"))?;
    assert_eq!(view.product_id, PRODUCT);
    assert_eq!(view.rating, 4);
    assert_eq!(view.status, ReviewStatus::Published);

    product_summary_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let rating = product_rating(&db, PRODUCT).await?;
    assert_eq!(rating.review_count, 2);
    assert_eq!(rating.average_rating, Some(4.5));

    let rejected = cmd.reject_review(&first, "already published".into()).await;
    assert!(matches!(rejected, Err(ReviewError::NotPending)));

    Ok(())
}

#[tokio::test]
async fn rejected_reviews_stay_out_of_the_rating() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(executor.clone());

    let id = cmd.submit_review(review("paul", 1)).await?;
    cmd.reject_review(&id, "insulting".into()).await?;

    let view = load_review_details(&executor, &id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("review view missing"))?;
    assert_eq!(view.status, ReviewStatus::Rejected);
    assert_eq!(view.rejection_reason.as_deref(), Some("insulting"));

    product_summary_subscription()
        .data(db.clone())
        .run_once(&executor)
        .await?;
    let rating = product_rating(&db, PRODUCT).await?;
    assert_eq!(rating.review_count, 0);
    assert_eq!(rating.average_rating, None);

    Ok(())
}

#[tokio::test]
async fn questions_collect_answers() -> anyhow::Result<()> {
    let (executor, _db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(executor.clone());

    let id = cmd
        .ask_question(AskQuestion {
            product_id: PRODUCT.into(),
            customer_id: "jonathan".into(),
            body: "Compatible G-SYNC ?".into(),
        })
        .await?;
    let unanswered = cmd
        .load_question(&id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("question missing"))?;
    assert!(!unanswered.is_answered());

    cmd.answer_question(&id, AnswerAuthor::Staff, "Oui, G-SYNC Compatible.".into())
        .await?;
    cmd.answer_question(
        &id,
        AnswerAuthor::Customer {
            customer_id: "marie".into(),
        },
        "Confirmé sur RTX 4070.".into(),
    )
    .await?;

    let answered = cmd
        .load_question(&id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("question missing"))?;
    assert!(answered.is_answered());
    assert_eq!(answered.answer_count, 2);
    assert_eq!(answered.product_id, PRODUCT);

    let missing = cmd
        .answer_question("nope", AnswerAuthor::Staff, "…".into())
        .await;
    assert!(matches!(missing, Err(ReviewError::QuestionNotFound)));

    Ok(())
}
