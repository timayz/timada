//! What the e-mails say is the host's to decide. [`Templates`] has one method
//! per e-mail, each defaulting to the built-in French text: a host implements
//! the trait, overrides the e-mails it wants — another language, its own
//! voice, an HTML alternative — and hands it to the mailer subscription as
//! [`MailerTemplates`] data. Without one, the built-ins are used.

use std::sync::Arc;

use timada_core::Money;
use timada_order::OrderDetailsView;
use timada_returns::ReturnView;

use crate::{config::MailerConfig, template};

/// What a template writes. The plain-text body is always there; `html_body`
/// is an alternative for mail clients that show HTML.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Content {
    pub subject: String,
    pub body: String,
    pub html_body: Option<String>,
}

impl Content {
    pub fn text(subject: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            subject: subject.into(),
            body: body.into(),
            html_body: None,
        }
    }

    pub fn with_html(mut self, html_body: impl Into<String>) -> Self {
        self.html_body = Some(html_body.into());
        self
    }
}

impl From<(String, String)> for Content {
    fn from((subject, body): (String, String)) -> Self {
        Self::text(subject, body)
    }
}

/// One method per e-mail the mailer sends. `first_name` is the customer's.
pub trait Templates: Send + Sync {
    fn welcome(&self, config: &MailerConfig, first_name: &str) -> Content {
        template::welcome(config, first_name).into()
    }

    fn order_confirmation(
        &self,
        config: &MailerConfig,
        first_name: &str,
        order: &OrderDetailsView,
    ) -> Content {
        template::order_confirmation(config, first_name, order).into()
    }

    fn order_shipped(
        &self,
        config: &MailerConfig,
        first_name: &str,
        order: &OrderDetailsView,
    ) -> Content {
        template::order_shipped(config, first_name, order).into()
    }

    /// `reason` is the order's cancellation reason, as recorded.
    fn order_cancelled(
        &self,
        config: &MailerConfig,
        first_name: &str,
        order: &OrderDetailsView,
        reason: &str,
    ) -> Content {
        template::order_cancelled(config, first_name, order, reason).into()
    }

    fn refund(
        &self,
        config: &MailerConfig,
        first_name: &str,
        order: &OrderDetailsView,
        amount: &Money,
    ) -> Content {
        template::refund(config, first_name, order, amount).into()
    }

    /// Sent to the shop ([`MailerConfig::alerts_to`]), when a dispute opens
    /// and again when the bank decided: `dispute.status` says which.
    fn payment_disputed(
        &self,
        config: &MailerConfig,
        order: &OrderDetailsView,
        dispute: &timada_payment::DisputeView,
    ) -> Content {
        template::payment_disputed(config, order, dispute).into()
    }

    /// Sent to the address left with the alert: there is no name to greet.
    fn back_in_stock(
        &self,
        config: &MailerConfig,
        product_id: &str,
        product_name: &str,
    ) -> Content {
        template::back_in_stock(config, product_id, product_name).into()
    }

    fn question_answered(
        &self,
        config: &MailerConfig,
        first_name: &str,
        product_id: &str,
        product_name: &str,
        question: &str,
        answer: &str,
    ) -> Content {
        template::question_answered(
            config,
            first_name,
            product_id,
            product_name,
            question,
            answer,
        )
        .into()
    }

    fn question_refused(
        &self,
        config: &MailerConfig,
        first_name: &str,
        product_name: &str,
        question: &str,
        reason: &str,
    ) -> Content {
        template::question_refused(config, first_name, product_name, question, reason).into()
    }

    fn review_published(
        &self,
        config: &MailerConfig,
        first_name: &str,
        product_id: &str,
        product_name: &str,
    ) -> Content {
        template::review_published(config, first_name, product_id, product_name).into()
    }

    fn review_rejected(
        &self,
        config: &MailerConfig,
        first_name: &str,
        product_name: &str,
        reason: &str,
    ) -> Content {
        template::review_rejected(config, first_name, product_name, reason).into()
    }

    fn return_approved(
        &self,
        config: &MailerConfig,
        first_name: &str,
        request: &ReturnView,
    ) -> Content {
        template::return_approved(config, first_name, request).into()
    }

    fn return_refused(
        &self,
        config: &MailerConfig,
        first_name: &str,
        request: &ReturnView,
    ) -> Content {
        template::return_refused(config, first_name, request).into()
    }

    fn return_completed(
        &self,
        config: &MailerConfig,
        first_name: &str,
        request: &ReturnView,
    ) -> Content {
        template::return_completed(config, first_name, request).into()
    }

    /// The prepaid label given *after* the approval e-mail went out; one
    /// given with the approval is announced by [`Self::return_approved`].
    /// Goes out with the label attached when the shop holds its file.
    fn return_label(
        &self,
        config: &MailerConfig,
        first_name: &str,
        request: &ReturnView,
    ) -> Content {
        template::return_label(config, first_name, request).into()
    }

    /// The parcel replacing a return's articles left the warehouse.
    fn replacement_shipped(
        &self,
        config: &MailerConfig,
        first_name: &str,
        request: &ReturnView,
        carrier: &str,
        tracking_number: &str,
    ) -> Content {
        template::replacement_shipped(config, first_name, request, carrier, tracking_number).into()
    }

    /// Goes out with the invoice attached as a PDF.
    #[cfg(feature = "invoice-pdf")]
    fn invoice_issued(
        &self,
        config: &MailerConfig,
        first_name: &str,
        invoice: &timada_invoice::InvoiceDocument,
    ) -> Content {
        template::invoice_issued(config, first_name, invoice).into()
    }

    /// Goes out with the credit note attached as a PDF, next to the refund
    /// e-mail.
    #[cfg(feature = "invoice-pdf")]
    fn credit_note_issued(
        &self,
        config: &MailerConfig,
        first_name: &str,
        credit_note: &timada_invoice::CreditNoteDocument,
    ) -> Content {
        template::credit_note_issued(config, first_name, credit_note).into()
    }
}

/// The built-in French e-mails, unchanged.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrenchTemplates;

impl Templates for FrenchTemplates {}

/// Subscription data: `.data(MailerTemplates::new(MyTemplates))`.
#[derive(Clone)]
pub struct MailerTemplates(pub Arc<dyn Templates>);

impl MailerTemplates {
    pub fn new(templates: impl Templates + 'static) -> Self {
        Self(Arc::new(templates))
    }
}

impl Default for MailerTemplates {
    fn default() -> Self {
        Self::new(FrenchTemplates)
    }
}
