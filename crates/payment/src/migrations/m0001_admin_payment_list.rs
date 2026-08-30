use sqlx_migrator::{sqlite_migration, vec_box};

pub struct M0001AdminPaymentList;

sqlite_migration!(
    M0001AdminPaymentList,
    "payment",
    "m0001_admin_payment_list",
    vec_box![],
    vec_box![
        (
            "CREATE TABLE admin_payment_list (
                 id TEXT PRIMARY KEY NOT NULL,
                 order_id TEXT NOT NULL,
                 provider TEXT NOT NULL,
                 amount_cents INTEGER NOT NULL,
                 currency TEXT NOT NULL,
                 provider_charge_ref TEXT,
                 status TEXT NOT NULL,
                 reason TEXT,
                 created_at INTEGER NOT NULL
             )",
            "DROP TABLE admin_payment_list"
        ),
        (
            "CREATE INDEX idx_admin_payment_list_recent
                 ON admin_payment_list (created_at DESC, id DESC)",
            "DROP INDEX idx_admin_payment_list_recent"
        )
    ]
);
