use timada_review::{
    AnswerAuthor, AskQuestion, Command, ListReviews, QuestionFilter, ReviewError, ReviewStatus,
    SubmitReview, answers_of_questions, count_published_questions, count_questions, count_reviews,
    list_questions, list_reviews, load_review_details, migrations, own_unpublished_questions,
    product_rating, product_summary_subscription, published_questions, published_reviews,
    question_list_subscription, review_list_subscription,
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
    let cmd = Command(&executor);

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
    let cmd = Command(&executor);

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
async fn the_review_list_feeds_the_product_page_and_the_moderation_queue() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

    let kept = cmd.submit_review(review("jonathan", 4)).await?;
    let refused = cmd
        .submit_review(SubmitReview {
            order_id: None,
            ..review("paul", 1)
        })
        .await?;
    let waiting = cmd.submit_review(review("marie", 5)).await?;
    cmd.publish_review(&kept).await?;
    cmd.reject_review(&refused, "insulting".into()).await?;
    for _ in 0..2 {
        review_list_subscription()
            .data(db.clone())
            .run_once(&executor)
            .await?;
    }

    // The product page only shows what moderation let through.
    let shown = published_reviews(&db, PRODUCT, 10, 0).await?;
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].review_id, kept);
    assert_eq!(shown[0].title, "Construisez vos victoires");
    assert!(shown[0].verified_purchase);
    assert!(published_reviews(&db, "other", 10, 0).await?.is_empty());

    // The queue: everything, or one status.
    assert_eq!(count_reviews(&db, None).await?, 3);
    let pending = ListReviews {
        status: Some(ReviewStatus::Pending),
        ..ListReviews::default()
    };
    let queue = list_reviews(&db, &pending).await?;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].review_id, waiting);
    let rejected = ListReviews {
        status: Some(ReviewStatus::Rejected),
        ..ListReviews::default()
    };
    let rows = list_reviews(&db, &rejected).await?;
    assert_eq!(rows[0].rejection_reason.as_deref(), Some("insulting"));
    assert!(!rows[0].verified_purchase);
    assert_eq!(count_reviews(&db, Some(&ReviewStatus::Rejected)).await?, 1);
    Ok(())
}

#[tokio::test]
async fn questions_collect_answers() -> anyhow::Result<()> {
    let (executor, db) = timada_core::testing::memory_executor(migrations()).await?;
    let cmd = Command(&executor);

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

    // A second question: it awaits moderation, so only its author sees it.
    let waiting = cmd
        .ask_question(AskQuestion {
            product_id: PRODUCT.into(),
            customer_id: "paul".into(),
            body: "Pied réglable en hauteur ?".into(),
        })
        .await?;
    // A third one is refused.
    let refused = cmd
        .ask_question(AskQuestion {
            product_id: PRODUCT.into(),
            customer_id: "paul".into(),
            body: "spam".into(),
        })
        .await?;
    cmd.reject_question(&refused, "Hors sujet".into()).await?;
    let sync = || async {
        for _ in 0..2 {
            question_list_subscription()
                .data(db.clone())
                .run_once(&executor)
                .await?;
        }
        anyhow::Ok(())
    };
    sync().await?;

    // The shop's answer published the first question; the two answers given
    // through `answer_question` are public as written.
    let shown = published_questions(&db, PRODUCT, 10, 0).await?;
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].question_id, id);
    assert_eq!(shown[0].answer_count, 2);
    assert_eq!(count_published_questions(&db, PRODUCT).await?, 1);
    let own = own_unpublished_questions(&db, PRODUCT, "paul").await?;
    assert_eq!(own.len(), 2);
    assert!(
        own.iter().any(
            |q| q.question_id == refused && q.rejection_reason.as_deref() == Some("Hors sujet")
        )
    );
    assert!(
        own_unpublished_questions(&db, PRODUCT, "jonathan")
            .await?
            .is_empty()
    );

    // A customer cannot answer what is not published.
    let too_early = cmd.submit_answer(&waiting, "marie", "Oui.".into()).await;
    assert!(matches!(too_early, Err(ReviewError::QuestionNotPublished)));
    cmd.publish_question(&waiting).await?;
    cmd.publish_question(&waiting).await?;
    assert!(matches!(
        cmd.reject_question(&waiting, "trop tard".into()).await,
        Err(ReviewError::QuestionNotPending)
    ));

    // Published, the question can be answered — once per customer — and the
    // answer waits for moderation.
    let helpful = cmd
        .submit_answer(&waiting, "marie", "Oui, sur 13 cm.".into())
        .await?;
    assert!(matches!(
        cmd.submit_answer(&waiting, "marie", "Encore moi.".into())
            .await,
        Err(ReviewError::AlreadyAnswered)
    ));
    let rude = cmd
        .submit_answer(&waiting, "jonathan", "Cherche sur Google.".into())
        .await?;
    sync().await?;
    assert!(
        answers_of_questions(&db, std::slice::from_ref(&waiting), true)
            .await?
            .is_empty()
    );
    assert_eq!(
        answers_of_questions(&db, std::slice::from_ref(&waiting), false)
            .await?
            .len(),
        2
    );
    // Both the question (its answers) and nothing else are to review.
    let queue = list_questions(&db, QuestionFilter::ToReview, 10, 0).await?;
    assert_eq!(queue.len(), 1);
    assert_eq!(queue[0].question_id, waiting);
    assert_eq!(count_questions(&db, QuestionFilter::Unanswered).await?, 1);

    cmd.publish_answer(&waiting, &helpful).await?;
    cmd.reject_answer(&waiting, &rude, "Désobligeant".into())
        .await?;
    assert!(matches!(
        cmd.publish_answer(&waiting, &rude).await,
        Err(ReviewError::AnswerNotPending)
    ));
    assert!(matches!(
        cmd.publish_answer(&waiting, "nope").await,
        Err(ReviewError::AnswerNotFound)
    ));
    sync().await?;
    let public = answers_of_questions(&db, std::slice::from_ref(&waiting), true).await?;
    assert_eq!(public.len(), 1);
    assert_eq!(public[0].body, "Oui, sur 13 cm.");
    assert_eq!(public[0].author_customer_id.as_deref(), Some("marie"));
    assert!(
        list_questions(&db, QuestionFilter::ToReview, 10, 0)
            .await?
            .is_empty()
    );
    assert_eq!(count_questions(&db, QuestionFilter::Answered).await?, 2);
    assert_eq!(count_questions(&db, QuestionFilter::Rejected).await?, 1);
    assert_eq!(count_questions(&db, QuestionFilter::All).await?, 3);

    // The shop cannot answer a refused question.
    assert!(matches!(
        cmd.answer_question(&refused, AnswerAuthor::Staff, "…".into())
            .await,
        Err(ReviewError::QuestionNotPublished)
    ));

    Ok(())
}
