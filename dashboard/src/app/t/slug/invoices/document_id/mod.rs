//! `/t/{slug}/invoices/{document_id}`: one issued document.

pub mod pdf;

use topcoat::router::module_param;

module_param!(pub document_id);
