use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001ReviewProductReview;

sqlite_migration!(
    M0001ReviewProductReview,
    "review",
    "m0001_review_product_review",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE review_product_review (
                review_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                rating INTEGER NOT NULL,
                published INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE review_product_review"
        ),
        (
            "CREATE INDEX review_product_review_product
                ON review_product_review (product_id, published)",
            "DROP INDEX review_product_review_product"
        )
    ]
);

pub struct M0002ReviewList;

sqlite_migration!(
    M0002ReviewList,
    "review",
    "m0002_review_list",
    vec_box![M0001ReviewProductReview],
    vec_box![
        (
            "CREATE TABLE review_list (
                review_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                verified_purchase INTEGER NOT NULL,
                rating INTEGER NOT NULL,
                title TEXT NOT NULL,
                body TEXT NOT NULL,
                status TEXT NOT NULL,
                rejection_reason TEXT,
                submitted_at INTEGER NOT NULL
            )",
            "DROP TABLE review_list"
        ),
        (
            "CREATE INDEX review_list_product ON review_list (product_id, status, submitted_at)",
            "DROP INDEX review_list_product"
        ),
        (
            "CREATE INDEX review_list_status ON review_list (status, submitted_at)",
            "DROP INDEX review_list_status"
        )
    ]
);

pub struct M0003QuestionList;

sqlite_migration!(
    M0003QuestionList,
    "review",
    "m0003_question_list",
    vec_box![M0002ReviewList],
    vec_box![
        (
            "CREATE TABLE review_question_list (
                question_id TEXT PRIMARY KEY,
                product_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                body TEXT NOT NULL,
                asked_at INTEGER NOT NULL,
                answer_count INTEGER NOT NULL DEFAULT 0
            )",
            "DROP TABLE review_question_list"
        ),
        (
            "CREATE INDEX review_question_list_product
             ON review_question_list (product_id, answer_count, asked_at)",
            "DROP INDEX review_question_list_product"
        ),
        (
            "CREATE INDEX review_question_list_queue
             ON review_question_list (answer_count, asked_at)",
            "DROP INDEX review_question_list_queue"
        ),
        (
            "CREATE TABLE review_answer_list (
                answer_id TEXT PRIMARY KEY,
                question_id TEXT NOT NULL,
                author_customer_id TEXT,
                body TEXT NOT NULL,
                answered_at INTEGER NOT NULL
            )",
            "DROP TABLE review_answer_list"
        ),
        (
            "CREATE INDEX review_answer_list_question
             ON review_answer_list (question_id, answered_at)",
            "DROP INDEX review_answer_list_question"
        )
    ]
);

pub struct M0004QuestionModeration;

// Questions and customers' answers are moderated. What was public before —
// a question with an answer, and every answer — stays public.
sqlite_migration!(
    M0004QuestionModeration,
    "review",
    "m0004_question_moderation",
    vec_box![M0003QuestionList],
    vec_box![
        (
            "ALTER TABLE review_question_list ADD COLUMN status TEXT NOT NULL DEFAULT 'pending'",
            "ALTER TABLE review_question_list DROP COLUMN status"
        ),
        (
            "ALTER TABLE review_question_list ADD COLUMN rejection_reason TEXT",
            "ALTER TABLE review_question_list DROP COLUMN rejection_reason"
        ),
        (
            "UPDATE review_question_list SET status = 'published' WHERE answer_count > 0",
            "SELECT 1"
        ),
        (
            "ALTER TABLE review_answer_list ADD COLUMN status TEXT NOT NULL DEFAULT 'published'",
            "ALTER TABLE review_answer_list DROP COLUMN status"
        ),
        (
            "ALTER TABLE review_answer_list ADD COLUMN rejection_reason TEXT",
            "ALTER TABLE review_answer_list DROP COLUMN rejection_reason"
        ),
        (
            "CREATE INDEX review_question_list_status
             ON review_question_list (status, asked_at)",
            "DROP INDEX review_question_list_status"
        ),
        (
            "CREATE INDEX review_answer_list_status ON review_answer_list (status, answered_at)",
            "DROP INDEX review_answer_list_status"
        )
    ]
);

/// Read-model migrations for this context, to register alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![
        M0001ReviewProductReview,
        M0002ReviewList,
        M0003QuestionList,
        M0004QuestionModeration
    ]
}
