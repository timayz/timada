//! The words of the automation pages: what each trigger, step, field and
//! problem is called. The crate names kinds by key; here they read.

use timada_automations::catalog::{
    self, CONDITION, CodeValue, DISCOUNT, EMAIL, FieldType, ISSUE_INVALID, ISSUE_MISSING,
    ISSUE_NO_RULES, ISSUE_NO_SUBJECT, ISSUE_TOO_MANY_RULES, ISSUE_UNKNOWN_FIELD, NOTIFY, Op,
    RuleValue, Subject, Unit, WAIT,
};
use timada_automations::{
    EditError, Outcome, PROBLEM_AFTER_BRANCH, PROBLEM_BRANCHES, PROBLEM_INVALID_ID,
    PROBLEM_NEWER_SCHEMA, PROBLEM_NO_STEPS, PROBLEM_NO_TRIGGER, PROBLEM_TOO_DEEP,
    PROBLEM_TOO_LARGE, PROBLEM_TOO_MANY_STEPS, PROBLEM_UNKNOWN_STEP, PROBLEM_UNKNOWN_TRIGGER,
    Problem, RunStatus, ScenarioStatus, Step,
};
use topcoat::icon::IconData;

use crate::ui::{Tone, icons};

/// The groups the trigger picker lists the triggers in.
pub const TRIGGER_GROUPS: [(&str, &[&str]); 3] = [
    (
        "Orders",
        &[
            catalog::ORDER_PLACED,
            catalog::ORDER_FULFILLED,
            catalog::ORDER_CANCELLED,
            catalog::ORDER_REFUNDED,
        ],
    ),
    (
        "Customers",
        &[catalog::SHOPPER_REGISTERED, catalog::SUBSCRIBER_CONFIRMED],
    ),
    (
        "Reviews and messages",
        &[
            catalog::REVIEW_SUBMITTED,
            catalog::REVIEW_PUBLISHED,
            catalog::MESSAGE_RECEIVED,
        ],
    ),
];

pub fn status_label(status: ScenarioStatus) -> &'static str {
    match status {
        ScenarioStatus::Draft => "Draft",
        ScenarioStatus::Active => "Active",
        ScenarioStatus::Paused => "Paused",
        ScenarioStatus::Archived => "Deleted",
    }
}

pub fn status_tone(status: ScenarioStatus) -> Tone {
    match status {
        ScenarioStatus::Draft | ScenarioStatus::Archived => Tone::Neutral,
        ScenarioStatus::Active => Tone::Success,
        ScenarioStatus::Paused => Tone::Warning,
    }
}

/// What a trigger reads as on its card: "When …".
pub fn trigger_label(kind: &str) -> &'static str {
    match kind {
        catalog::ORDER_PLACED => "When an order is placed",
        catalog::ORDER_FULFILLED => "When an order is shipped",
        catalog::ORDER_CANCELLED => "When an order is cancelled",
        catalog::ORDER_REFUNDED => "When money goes back on an order",
        catalog::SHOPPER_REGISTERED => "When a customer makes an account",
        catalog::SUBSCRIBER_CONFIRMED => "When someone joins the newsletter",
        catalog::REVIEW_SUBMITTED => "When a review is submitted",
        catalog::REVIEW_PUBLISHED => "When a review is published",
        catalog::MESSAGE_RECEIVED => "When a contact message arrives",
        _ => "An unknown trigger",
    }
}

pub fn trigger_description(kind: &str) -> &'static str {
    match kind {
        catalog::ORDER_PLACED => "A shopper paid for an order.",
        catalog::ORDER_FULFILLED => "The order was marked shipped.",
        catalog::ORDER_CANCELLED => "The order was cancelled (and refunded).",
        catalog::ORDER_REFUNDED => {
            "Once per refund: a partial refund counts, and cancelling an order refunds it too."
        }
        catalog::SHOPPER_REGISTERED => "A customer made an account with the shop.",
        catalog::SUBSCRIBER_CONFIRMED => {
            "An address joined the newsletter, its owner having followed the mailed link."
        }
        catalog::REVIEW_SUBMITTED => "A buyer wrote a review; it waits for moderation.",
        catalog::REVIEW_PUBLISHED => "A review went public.",
        catalog::MESSAGE_RECEIVED => "A visitor wrote through the contact form.",
        _ => "This build does not know this trigger.",
    }
}

pub fn run_status_label(status: RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "Running",
        RunStatus::Waiting => "Waiting",
        RunStatus::Finished => "Finished",
        RunStatus::Failed => "Failed",
        RunStatus::Cancelled => "Cancelled",
    }
}

pub fn run_status_tone(status: RunStatus) -> Tone {
    match status {
        RunStatus::Running => Tone::Primary,
        RunStatus::Waiting => Tone::Neutral,
        RunStatus::Finished => Tone::Success,
        RunStatus::Failed => Tone::Danger,
        RunStatus::Cancelled => Tone::Warning,
    }
}

/// What a step of a run did, in a few words.
pub fn outcome_label(outcome: &Outcome, kind: &str) -> String {
    match outcome {
        Outcome::Waiting { until } => format!("Waits until {}", crate::ui::format_time(*until)),
        Outcome::Branched(branch) => {
            format!("Went down {}", branch_label(kind, *branch as usize))
        }
        Outcome::Claimed { attempt: 1 } => "Taken to act".to_owned(),
        Outcome::Claimed { attempt } => format!("Taken to act (attempt {attempt})"),
        Outcome::Succeeded { .. } => "Done".to_owned(),
        Outcome::Skipped { reason } => format!("Skipped: {reason}"),
        Outcome::Failed {
            reason,
            retry_at: Some(at),
            ..
        } => format!(
            "Failed: {reason} Tried again at {}.",
            crate::ui::format_time(*at)
        ),
        Outcome::Failed { reason, .. } => format!("Failed: {reason}"),
    }
}

/// What a run is about, as a word.
pub fn subject_kind(subject: Subject) -> &'static str {
    match subject {
        Subject::Order => "Order",
        Subject::Shopper => "Customer",
        Subject::Subscriber => "Subscriber",
        Subject::Review => "Review",
        Subject::Message => "Message",
    }
}

/// What a run of a scenario with this subject is about, in a few words.
pub fn subject_label(subject: Subject) -> &'static str {
    match subject {
        Subject::Order => "the order",
        Subject::Shopper => "the customer",
        Subject::Subscriber => "the subscriber",
        Subject::Review => "the review",
        Subject::Message => "the message",
    }
}

pub fn step_label(kind: &str) -> &'static str {
    match kind {
        WAIT => "Wait",
        CONDITION => "If / else",
        NOTIFY => "Notify the team",
        EMAIL => "E-mail the customer",
        DISCOUNT => "Create a discount code",
        _ => "Unknown block",
    }
}

pub fn step_description(kind: &str) -> &'static str {
    match kind {
        WAIT => "Pause for minutes, hours or days before the next step.",
        CONDITION => "Check rules, then go down the Yes or the No lane, each to its own end.",
        NOTIFY => "Mail the members who may see what started the run.",
        EMAIL => {
            "Mail the buyer, customer or visitor the run is about, from the shop's own server."
        }
        DISCOUNT => "Make a single-use code that a mail after it can quote.",
        _ => "Made with a newer version: it is kept as it is.",
    }
}

pub fn step_icon(kind: &str) -> IconData {
    match kind {
        WAIT => icons::WAIT,
        CONDITION => icons::CONDITION,
        NOTIFY => icons::NOTIFY,
        EMAIL => icons::MAIL,
        DISCOUNT => icons::DISCOUNTS,
        _ => icons::BLOCK,
    }
}

/// The lanes of a branching step, by index.
pub fn branch_label(kind: &str, branch: usize) -> &'static str {
    match (kind, branch) {
        (CONDITION, 0) => "Yes",
        (CONDITION, 1) => "No",
        _ => "Branch",
    }
}

pub fn unit_label(unit: Unit, amount: u32) -> &'static str {
    match (unit, amount == 1) {
        (Unit::Minutes, true) => "minute",
        (Unit::Minutes, false) => "minutes",
        (Unit::Hours, true) => "hour",
        (Unit::Hours, false) => "hours",
        (Unit::Days, true) => "day",
        (Unit::Days, false) => "days",
    }
}

pub fn field_label(key: &str) -> &'static str {
    match key {
        "order.total" => "Order total",
        "order.items" => "Number of items",
        "order.country" => "Delivery country",
        "order.discounted" => "A discount code was used",
        "order.first" => "The buyer's first order",
        "order.fulfilled" => "Shipped",
        "order.refunded" => "Refunded",
        "order.cancelled" => "Cancelled",
        "shopper.has_ordered" => "Has ordered",
        "subscriber.language" => "Language",
        "subscriber.source" => "Signed up from",
        "subscriber.has_ordered" => "Has ordered",
        "review.rating" => "Rating",
        "review.photos" => "Has photos",
        "review.published" => "Published",
        "message.language" => "Language",
        "message.handled" => "Handled",
        _ => "Unknown",
    }
}

/// What a value of a field's type is written as, for the input's hint.
pub fn value_hint(ty: FieldType) -> &'static str {
    match ty {
        FieldType::Money => "An amount, like 49.90",
        FieldType::Number => "A whole number",
        FieldType::Text => "Some text (case does not matter)",
        FieldType::Country => "A country code, like FR",
        FieldType::Bool => "",
    }
}

pub fn op_label(op: Op) -> &'static str {
    match op {
        Op::Eq => "is",
        Op::Ne => "is not",
        Op::Gt => "is more than",
        Op::Ge => "is at least",
        Op::Lt => "is less than",
        Op::Le => "is at most",
        Op::Contains => "contains",
        Op::IsTrue => "yes",
        Op::IsFalse => "no",
    }
}

fn op_symbol(op: Op) -> &'static str {
    match op {
        Op::Eq => "=",
        Op::Ne => "≠",
        Op::Gt => ">",
        Op::Ge => "≥",
        Op::Lt => "<",
        Op::Le => "≤",
        Op::Contains => "contains",
        Op::IsTrue | Op::IsFalse => "",
    }
}

fn cents(amount: u64) -> String {
    format!("{}.{:02}", amount / 100, amount % 100)
}

/// One line saying what a step does, for its card.
pub fn step_summary(step: &Step, subject: Option<Subject>) -> String {
    match step.kind.as_str() {
        WAIT => match catalog::parse_wait(&step.settings) {
            Ok(wait) => format!(
                "Wait {} {}",
                wait.amount,
                unit_label(wait.unit, wait.amount)
            ),
            Err(_) => "Set how long to wait".to_owned(),
        },
        CONDITION => match catalog::parse_condition(subject, &step.settings) {
            Ok(condition) if condition.rules.len() == 1 => {
                let rule = &condition.rules[0];
                let field = field_label(rule.field.key);
                match &rule.value {
                    RuleValue::None if rule.op == Op::IsTrue => {
                        format!("If {}", field.to_lowercase())
                    }
                    RuleValue::None => format!("If not {}", field.to_lowercase()),
                    RuleValue::Cents(amount) => {
                        format!("If {field} {} {}", op_symbol(rule.op), cents(*amount))
                    }
                    RuleValue::Number(n) => format!("If {field} {} {n}", op_symbol(rule.op)),
                    RuleValue::Text(text) | RuleValue::Country(text) => {
                        format!("If {field} {} {text}", op_label(rule.op))
                    }
                }
            }
            Ok(condition) => format!(
                "If {} of {} rules hold",
                if condition.all { "all" } else { "any" },
                condition.rules.len()
            ),
            Err(_) => "Set the rules to check".to_owned(),
        },
        NOTIFY => match catalog::parse_notify(&step.settings) {
            Ok(notify) => format!("“{}”", notify.subject),
            Err(_) => "Write what the team reads".to_owned(),
        },
        EMAIL => match catalog::parse_email(&step.settings) {
            Ok(mail) => format!("“{}”", mail.subject),
            Err(_) => "Write the mail".to_owned(),
        },
        DISCOUNT => match catalog::parse_discount(&step.settings) {
            Ok(code) => {
                let value = match &code.value {
                    CodeValue::Percent(bps) => {
                        format!("{}.{:02} %", bps / 100, bps % 100).replace(".00 %", " %")
                    }
                    CodeValue::Amount { cents, currency } => {
                        timada_catalog::format_money(*cents, currency)
                    }
                };
                format!("{value} off, {} days", code.days)
            }
            Err(_) => "Set what the code takes off".to_owned(),
        },
        _ => "Kept as it is".to_owned(),
    }
}

/// What a problem says, near what it is about.
pub fn problem_label(problem: &Problem) -> &'static str {
    match problem.code {
        PROBLEM_NEWER_SCHEMA => "This scenario was drawn with a newer version and is shown only.",
        PROBLEM_NO_TRIGGER => "Choose what starts the scenario.",
        PROBLEM_UNKNOWN_TRIGGER => "This build does not know the trigger: choose another.",
        PROBLEM_NO_STEPS => "Add at least one step under the trigger.",
        PROBLEM_UNKNOWN_STEP => "This block was made with a newer version: remove it to publish.",
        PROBLEM_BRANCHES | PROBLEM_INVALID_ID => "This block is damaged: remove it.",
        PROBLEM_TOO_MANY_STEPS => "A scenario holds at most 50 steps.",
        PROBLEM_TOO_DEEP => "Conditions nest at most 4 deep.",
        PROBLEM_TOO_LARGE => "The scenario is too large.",
        PROBLEM_AFTER_BRANCH => "Nothing runs after an If / else: move this into one of its lanes.",
        ISSUE_MISSING => "Fill this in.",
        ISSUE_INVALID => "This does not read: check it.",
        ISSUE_UNKNOWN_FIELD => "The trigger's subject has no such field: choose another.",
        ISSUE_NO_RULES => "Add a rule.",
        ISSUE_TOO_MANY_RULES => "A condition checks at most 10 rules.",
        ISSUE_NO_SUBJECT => "Choose the trigger first: the rules depend on it.",
        _ => "Something needs setting up here.",
    }
}

/// Why an edit did not happen.
pub fn edit_error_label(error: EditError) -> &'static str {
    match error {
        EditError::NoSuchStep => "That block is not in the scenario any more.",
        EditError::NoSuchSlot => "That place is not in the scenario any more.",
        EditError::UnknownKind => "This build does not know that kind of block.",
        EditError::IntoItself => "A block cannot go inside itself.",
        EditError::TooManySteps => "A scenario holds at most 50 steps.",
        EditError::TooDeep => "Conditions nest at most 4 deep.",
        EditError::TooManyRules => "A condition checks at most 10 rules.",
        EditError::NewerSchema => "This scenario was drawn with a newer version and is shown only.",
        EditError::AfterBranch => "Nothing comes after an If / else: put it in one of its lanes.",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn step(kind: &str, pairs: &[(&str, &str)]) -> Step {
        Step {
            id: "s1".into(),
            kind: kind.into(),
            settings: pairs
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
                .collect::<BTreeMap<_, _>>(),
            branches: Vec::new(),
        }
    }

    #[test]
    fn a_card_says_what_its_step_does() {
        let wait = step(WAIT, &[("amount", "2"), ("unit", "days")]);
        assert_eq!(step_summary(&wait, None), "Wait 2 days");
        let wait = step(WAIT, &[("amount", "1"), ("unit", "hours")]);
        assert_eq!(step_summary(&wait, None), "Wait 1 hour");
        let condition = step(
            CONDITION,
            &[
                ("rules.0.field", "order.total"),
                ("rules.0.op", "ge"),
                ("rules.0.value", "50"),
            ],
        );
        assert_eq!(
            step_summary(&condition, Some(Subject::Order)),
            "If Order total ≥ 50.00"
        );
        let condition = step(
            CONDITION,
            &[("rules.0.field", "order.first"), ("rules.0.op", "true")],
        );
        assert_eq!(
            step_summary(&condition, Some(Subject::Order)),
            "If the buyer's first order"
        );
        let notify = step(NOTIFY, &[("subject", "Big order")]);
        assert_eq!(step_summary(&notify, None), "“Big order”");
        assert_eq!(step_summary(&step("teleport", &[]), None), "Kept as it is");
    }

    #[test]
    fn every_known_kind_has_its_words() {
        for kind in catalog::TRIGGERS {
            assert_ne!(trigger_label(kind.key), "An unknown trigger");
            assert!(
                TRIGGER_GROUPS
                    .iter()
                    .any(|(_, kinds)| kinds.contains(&kind.key)),
                "{} is in no group",
                kind.key
            );
        }
        for kind in catalog::STEPS {
            assert_ne!(step_label(kind.key), "Unknown block");
            assert_ne!(
                step_description(kind.key),
                step_description("unknown"),
                "{} has no description",
                kind.key
            );
        }
        for subject in [
            Subject::Order,
            Subject::Shopper,
            Subject::Subscriber,
            Subject::Review,
            Subject::Message,
        ] {
            for field in subject.fields() {
                assert_ne!(field_label(field.key), "Unknown", "{}", field.key);
            }
        }
    }
}
