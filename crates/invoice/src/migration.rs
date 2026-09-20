use sqlx::Sqlite;
use sqlx_migrator::{Migration, sqlite_migration, vec_box};

pub struct M0001InvoiceNumber;

sqlite_migration!(
    M0001InvoiceNumber,
    "invoice",
    "m0001_invoice_number",
    vec_box![],
    vec_box![(
        "CREATE TABLE invoice_number (
            order_id TEXT PRIMARY KEY,
            number INTEGER NOT NULL UNIQUE
        )",
        "DROP TABLE invoice_number"
    )]
);

pub struct M0002InvoiceList;

sqlite_migration!(
    M0002InvoiceList,
    "invoice",
    "m0002_invoice_list",
    vec_box![M0001InvoiceNumber],
    vec_box![
        (
            "CREATE TABLE invoice_list (
                invoice_id TEXT PRIMARY KEY,
                order_id TEXT NOT NULL,
                customer_id TEXT NOT NULL,
                invoice_number TEXT,
                status TEXT NOT NULL,
                total_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                drafted_at INTEGER NOT NULL,
                issued_at INTEGER
            )",
            "DROP TABLE invoice_list"
        ),
        (
            "CREATE INDEX invoice_list_status ON invoice_list (status, drafted_at)",
            "DROP INDEX invoice_list_status"
        )
    ]
);

pub struct M0003CreditNoteNumber;

sqlite_migration!(
    M0003CreditNoteNumber,
    "invoice",
    "m0003_credit_note_number",
    vec_box![M0002InvoiceList],
    vec_box![
        (
            "CREATE TABLE credit_note_number (
                refund_id TEXT PRIMARY KEY,
                number INTEGER NOT NULL UNIQUE,
                invoice_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL
            )",
            "DROP TABLE credit_note_number"
        ),
        (
            "CREATE INDEX credit_note_number_invoice ON credit_note_number (invoice_id)",
            "DROP INDEX credit_note_number_invoice"
        )
    ]
);

pub struct M0004CreditNoteList;

sqlite_migration!(
    M0004CreditNoteList,
    "invoice",
    "m0004_credit_note_list",
    vec_box![M0003CreditNoteNumber],
    vec_box![
        (
            "CREATE TABLE invoice_credit_note_list (
                credit_note_id TEXT PRIMARY KEY,
                credit_note_number TEXT NOT NULL,
                refund_id TEXT NOT NULL UNIQUE,
                invoice_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                amount_minor INTEGER NOT NULL,
                currency TEXT NOT NULL,
                reason TEXT NOT NULL,
                issued_at INTEGER NOT NULL
            )",
            "DROP TABLE invoice_credit_note_list"
        ),
        (
            "CREATE INDEX invoice_credit_note_list_invoice
             ON invoice_credit_note_list (invoice_id, issued_at)",
            "DROP INDEX invoice_credit_note_list_invoice"
        )
    ]
);

pub struct M0005VatJournal;

sqlite_migration!(
    M0005VatJournal,
    "invoice",
    "m0005_vat_journal",
    vec_box![M0004CreditNoteList],
    vec_box![
        // A row per VAT rate of each issued invoice and — negative — of each
        // credit note. `rate_bp` is NULL for an invoice without a breakdown.
        (
            "CREATE TABLE invoice_vat_journal (
                document_kind TEXT NOT NULL,
                document_id TEXT NOT NULL,
                document_number TEXT NOT NULL,
                invoice_id TEXT NOT NULL,
                order_id TEXT NOT NULL,
                issued_at INTEGER NOT NULL,
                invoice_issued_at INTEGER NOT NULL,
                zone_code TEXT NOT NULL,
                treatment TEXT NOT NULL,
                country_code TEXT NOT NULL,
                rate_bp INTEGER,
                base_minor INTEGER NOT NULL,
                vat_minor INTEGER NOT NULL,
                currency TEXT NOT NULL
            )",
            "DROP TABLE invoice_vat_journal"
        ),
        (
            "CREATE INDEX invoice_vat_journal_period ON invoice_vat_journal (issued_at, treatment)",
            "DROP INDEX invoice_vat_journal_period"
        ),
        (
            "CREATE INDEX invoice_vat_journal_document ON invoice_vat_journal (document_id)",
            "DROP INDEX invoice_vat_journal_document"
        )
    ]
);

pub struct M0006VatJournalBuyer;

sqlite_migration!(
    M0006VatJournalBuyer,
    "invoice",
    "m0006_vat_journal_buyer",
    vec_box![M0005VatJournal],
    vec_box![(
        // Who a reverse-charged sale was made to: what the recapitulative
        // statement of customers lists.
        "ALTER TABLE invoice_vat_journal ADD COLUMN buyer_vat_number TEXT",
        "ALTER TABLE invoice_vat_journal DROP COLUMN buyer_vat_number"
    )]
);

/// Write-side and read-model migrations for this context, to register
/// alongside evento's.
pub fn migrations() -> Vec<Box<dyn Migration<Sqlite>>> {
    vec_box![
        M0001InvoiceNumber,
        M0002InvoiceList,
        M0003CreditNoteNumber,
        M0004CreditNoteList,
        M0005VatJournal,
        M0006VatJournalBuyer
    ]
}
