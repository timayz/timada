//! What a scenario may be made of: the kinds of trigger and of step this
//! build knows, what each trigger is about (its [`Subject`]), and what a
//! condition may look at. Kinds are the strings a definition stores; the
//! host words them.
//!
//! A step's settings are kept as typed (strings by key, see
//! [`Step::settings`](crate::Step::settings)); the `parse_*` functions here
//! read them into typed settings, or say which key is wrong and why.

use std::collections::BTreeMap;

use crate::value_object::{MAX_NOTE_LEN, MAX_RULES, MAX_SUBJECT_LEN, MAX_VALUE_LEN, MAX_WAIT_SECS};

/// What a run is about: the thing whose event started it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Subject {
    Order,
    /// A customer who made an account with the shop.
    Shopper,
    /// An address on the shop's newsletter.
    Subscriber,
    Review,
    /// A message sent through the shop's contact form.
    Message,
    /// A cart whose checkout was left unpaid.
    Cart,
}

impl Subject {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Order => "order",
            Self::Shopper => "shopper",
            Self::Subscriber => "subscriber",
            Self::Review => "review",
            Self::Message => "message",
            Self::Cart => "cart",
        }
    }

    /// What a condition may look at in a run about this subject.
    pub fn fields(self) -> &'static [Field] {
        match self {
            Self::Order => ORDER_FIELDS,
            Self::Shopper => SHOPPER_FIELDS,
            Self::Subscriber => SUBSCRIBER_FIELDS,
            Self::Review => REVIEW_FIELDS,
            Self::Message => MESSAGE_FIELDS,
            Self::Cart => CART_FIELDS,
        }
    }

    pub fn field(self, key: &str) -> Option<&'static Field> {
        self.fields().iter().find(|field| field.key == key)
    }
}

/// An order was placed: it is paid, as an order exists only once paid.
pub const ORDER_PLACED: &str = "order.placed";
/// An order was marked shipped (fulfilled).
pub const ORDER_FULFILLED: &str = "order.fulfilled";
pub const ORDER_CANCELLED: &str = "order.cancelled";
/// Money went back on an order: once per refund, so a partial refund
/// fires it as many times as there are refunds, and cancelling an order
/// (which refunds it) fires it next to [`ORDER_CANCELLED`].
pub const ORDER_REFUNDED: &str = "order.refunded";
/// A customer made an account with the shop.
pub const SHOPPER_REGISTERED: &str = "shopper.registered";
/// An address joined the newsletter, its owner having followed the link.
pub const SUBSCRIBER_CONFIRMED: &str = "subscriber.confirmed";
/// A buyer wrote a review; it waits for moderation.
pub const REVIEW_SUBMITTED: &str = "review.submitted";
pub const REVIEW_PUBLISHED: &str = "review.published";
/// A visitor sent a message through the contact form.
pub const MESSAGE_RECEIVED: &str = "message.received";
/// A shopper asked to pay for a cart and no order followed within the
/// operator's delay (an hour unless set otherwise).
pub const CHECKOUT_ABANDONED: &str = "checkout.abandoned";

/// A kind of trigger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TriggerKind {
    pub key: &'static str,
    pub subject: Subject,
}

/// Every trigger this build knows, in the order the editor offers them.
pub const TRIGGERS: &[TriggerKind] = &[
    TriggerKind {
        key: ORDER_PLACED,
        subject: Subject::Order,
    },
    TriggerKind {
        key: ORDER_FULFILLED,
        subject: Subject::Order,
    },
    TriggerKind {
        key: ORDER_CANCELLED,
        subject: Subject::Order,
    },
    TriggerKind {
        key: ORDER_REFUNDED,
        subject: Subject::Order,
    },
    TriggerKind {
        key: SHOPPER_REGISTERED,
        subject: Subject::Shopper,
    },
    TriggerKind {
        key: SUBSCRIBER_CONFIRMED,
        subject: Subject::Subscriber,
    },
    TriggerKind {
        key: REVIEW_SUBMITTED,
        subject: Subject::Review,
    },
    TriggerKind {
        key: REVIEW_PUBLISHED,
        subject: Subject::Review,
    },
    TriggerKind {
        key: MESSAGE_RECEIVED,
        subject: Subject::Message,
    },
    TriggerKind {
        key: CHECKOUT_ABANDONED,
        subject: Subject::Cart,
    },
];

pub fn trigger_kind(key: &str) -> Option<&'static TriggerKind> {
    TRIGGERS.iter().find(|kind| kind.key == key)
}

/// Waits a while before the next step.
pub const WAIT: &str = "wait";
/// Checks rules on the subject and goes on down its Yes lane or its No lane.
pub const CONDITION: &str = "condition";
/// Mails the shop's members a note about the subject.
pub const NOTIFY: &str = "notify";
/// Mails the customer the run is about (the buyer of an order, the visitor
/// who wrote, …) a message the merchant wrote.
pub const EMAIL: &str = "email";
/// Creates a single-use discount code, which a later mail may quote.
pub const DISCOUNT: &str = "discount";
/// Posts what started the run to an address of the merchant's, signed.
pub const WEBHOOK: &str = "webhook";

/// A kind of step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepKind {
    pub key: &'static str,
    /// How many lanes it holds: a condition two (Yes, No), the others none.
    pub branches: usize,
}

/// Every step this build knows, in the order the editor offers them.
pub const STEPS: &[StepKind] = &[
    StepKind {
        key: WAIT,
        branches: 0,
    },
    StepKind {
        key: CONDITION,
        branches: 2,
    },
    StepKind {
        key: NOTIFY,
        branches: 0,
    },
    StepKind {
        key: EMAIL,
        branches: 0,
    },
    StepKind {
        key: DISCOUNT,
        branches: 0,
    },
    StepKind {
        key: WEBHOOK,
        branches: 0,
    },
];

pub fn step_kind(key: &str) -> Option<&'static StepKind> {
    STEPS.iter().find(|kind| kind.key == key)
}

/// What a value compared in a condition is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    /// An amount in the order's currency, written `49.90` (or `49,90`).
    Money,
    /// A whole number.
    Number,
    /// Text, compared without regard to case.
    Text,
    /// An ISO 3166 alpha-2 country code.
    Country,
    /// Yes or no.
    Bool,
}

impl FieldType {
    /// The operators a rule on such a value may use.
    pub fn ops(self) -> &'static [Op] {
        match self {
            Self::Money | Self::Number => &[Op::Ge, Op::Gt, Op::Le, Op::Lt, Op::Eq, Op::Ne],
            Self::Text => &[Op::Eq, Op::Ne, Op::Contains],
            Self::Country => &[Op::Eq, Op::Ne],
            Self::Bool => &[Op::IsTrue, Op::IsFalse],
        }
    }
}

/// Something a condition may look at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Field {
    pub key: &'static str,
    pub ty: FieldType,
}

const ORDER_FIELDS: &[Field] = &[
    Field {
        key: "order.total",
        ty: FieldType::Money,
    },
    Field {
        key: "order.items",
        ty: FieldType::Number,
    },
    Field {
        key: "order.country",
        ty: FieldType::Country,
    },
    Field {
        key: "order.discounted",
        ty: FieldType::Bool,
    },
    // The buyer's first order with the shop (by address).
    Field {
        key: "order.first",
        ty: FieldType::Bool,
    },
    Field {
        key: "order.fulfilled",
        ty: FieldType::Bool,
    },
    Field {
        key: "order.refunded",
        ty: FieldType::Bool,
    },
    Field {
        key: "order.cancelled",
        ty: FieldType::Bool,
    },
];

const SHOPPER_FIELDS: &[Field] = &[Field {
    key: "shopper.has_ordered",
    ty: FieldType::Bool,
}];

const SUBSCRIBER_FIELDS: &[Field] = &[
    Field {
        key: "subscriber.language",
        ty: FieldType::Text,
    },
    // Where they asked: `form`, `checkout` or `account`.
    Field {
        key: "subscriber.source",
        ty: FieldType::Text,
    },
    Field {
        key: "subscriber.has_ordered",
        ty: FieldType::Bool,
    },
];

const REVIEW_FIELDS: &[Field] = &[
    Field {
        key: "review.rating",
        ty: FieldType::Number,
    },
    Field {
        key: "review.photos",
        ty: FieldType::Bool,
    },
    Field {
        key: "review.published",
        ty: FieldType::Bool,
    },
];

const CART_FIELDS: &[Field] = &[
    // What the shopper was about to pay.
    Field {
        key: "cart.total",
        ty: FieldType::Money,
    },
    Field {
        key: "cart.items",
        ty: FieldType::Number,
    },
    // Ordered since (a reminder after a wait checks it).
    Field {
        key: "cart.ordered",
        ty: FieldType::Bool,
    },
];

const MESSAGE_FIELDS: &[Field] = &[
    Field {
        key: "message.language",
        ty: FieldType::Text,
    },
    Field {
        key: "message.handled",
        ty: FieldType::Bool,
    },
];

/// How a rule compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
    Contains,
    IsTrue,
    IsFalse,
}

impl Op {
    pub const ALL: [Self; 9] = [
        Self::Eq,
        Self::Ne,
        Self::Gt,
        Self::Ge,
        Self::Lt,
        Self::Le,
        Self::Contains,
        Self::IsTrue,
        Self::IsFalse,
    ];

    /// The value stored in the settings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Eq => "eq",
            Self::Ne => "ne",
            Self::Gt => "gt",
            Self::Ge => "ge",
            Self::Lt => "lt",
            Self::Le => "le",
            Self::Contains => "contains",
            Self::IsTrue => "true",
            Self::IsFalse => "false",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|op| op.as_str() == value)
    }

    /// Whether the rule compares with a value at all.
    pub fn takes_value(self) -> bool {
        !matches!(self, Self::IsTrue | Self::IsFalse)
    }
}

/// A wait's unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Minutes,
    Hours,
    Days,
}

impl Unit {
    pub const ALL: [Self; 3] = [Self::Minutes, Self::Hours, Self::Days];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minutes => "minutes",
            Self::Hours => "hours",
            Self::Days => "days",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.as_str() == value)
    }

    pub fn secs(self) -> u64 {
        match self {
            Self::Minutes => 60,
            Self::Hours => 60 * 60,
            Self::Days => 24 * 60 * 60,
        }
    }
}

/// A setting that does not read: its key and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub key: String,
    pub code: &'static str,
}

/// A setting is empty.
pub const ISSUE_MISSING: &str = "missing";
/// A setting holds what it cannot.
pub const ISSUE_INVALID: &str = "invalid";
/// A condition looks at what the trigger's subject does not have.
pub const ISSUE_UNKNOWN_FIELD: &str = "unknown_field";
/// A condition has no rule.
pub const ISSUE_NO_RULES: &str = "no_rules";
/// A condition has more than [`MAX_RULES`] rules.
pub const ISSUE_TOO_MANY_RULES: &str = "too_many_rules";
/// A condition's subject is not known: the scenario has no trigger yet.
pub const ISSUE_NO_SUBJECT: &str = "no_subject";

fn issue(key: impl Into<String>, code: &'static str) -> Issue {
    Issue {
        key: key.into(),
        code,
    }
}

fn setting<'a>(settings: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    settings.get(key).map(|value| value.trim()).unwrap_or("")
}

/// A wait, read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitSettings {
    pub amount: u32,
    pub unit: Unit,
}

impl WaitSettings {
    pub fn secs(&self) -> u64 {
        u64::from(self.amount) * self.unit.secs()
    }
}

pub fn parse_wait(settings: &BTreeMap<String, String>) -> Result<WaitSettings, Vec<Issue>> {
    let mut issues = Vec::new();
    let amount = match setting(settings, "amount") {
        "" => {
            issues.push(issue("amount", ISSUE_MISSING));
            None
        }
        amount => match amount.parse::<u32>() {
            Ok(amount) if amount > 0 => Some(amount),
            _ => {
                issues.push(issue("amount", ISSUE_INVALID));
                None
            }
        },
    };
    let unit = match Unit::parse(setting(settings, "unit")) {
        Some(unit) => Some(unit),
        None => {
            issues.push(issue("unit", ISSUE_INVALID));
            None
        }
    };
    match (amount, unit) {
        (Some(amount), Some(unit)) => {
            let wait = WaitSettings { amount, unit };
            if wait.secs() > MAX_WAIT_SECS {
                return Err(vec![issue("amount", ISSUE_INVALID)]);
            }
            Ok(wait)
        }
        _ => Err(issues),
    }
}

/// What a rule compares with, read for its field's type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleValue {
    /// The rule takes none (yes/no fields).
    None,
    Cents(u64),
    Number(u64),
    /// Lowercased.
    Text(String),
    /// Uppercased.
    Country(String),
}

/// One rule of a condition, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub field: &'static Field,
    pub op: Op,
    pub value: RuleValue,
}

/// A condition, read: its rules, and whether all of them must hold (or any).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionSettings {
    pub all: bool,
    pub rules: Vec<Rule>,
}

/// The rule indices a condition's settings hold (`rules.<i>.…`), in order.
pub fn rule_indices(settings: &BTreeMap<String, String>) -> Vec<usize> {
    let mut indices: Vec<usize> = settings
        .keys()
        .filter_map(|key| key.strip_prefix("rules."))
        .filter_map(|rest| rest.split_once('.'))
        .filter_map(|(index, _)| index.parse().ok())
        .collect();
    indices.sort_unstable();
    indices.dedup();
    indices
}

/// Reads an amount written `49.90`, `49,90` or `49` into cents.
pub fn parse_cents(value: &str) -> Option<u64> {
    let value = value.trim().replace(',', ".");
    let (whole, fraction) = value.split_once('.').unwrap_or((&value, ""));
    if whole.is_empty() || fraction.len() > 2 || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: u64 = whole.parse().ok()?;
    let fraction: u64 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    whole.checked_mul(100)?.checked_add(fraction)
}

fn parse_value(ty: FieldType, value: &str) -> Option<RuleValue> {
    let value = value.trim();
    match ty {
        FieldType::Bool => Some(RuleValue::None),
        FieldType::Money => parse_cents(value).map(RuleValue::Cents),
        FieldType::Number => value.parse().ok().map(RuleValue::Number),
        FieldType::Text => (!value.is_empty()
            && value.chars().count() <= MAX_VALUE_LEN
            && !value.chars().any(char::is_control))
        .then(|| RuleValue::Text(value.to_lowercase())),
        FieldType::Country => (value.len() == 2 && value.bytes().all(|b| b.is_ascii_alphabetic()))
            .then(|| RuleValue::Country(value.to_ascii_uppercase())),
    }
}

/// Reads a condition for a run about `subject` (`None` while the scenario
/// has no trigger).
pub fn parse_condition(
    subject: Option<Subject>,
    settings: &BTreeMap<String, String>,
) -> Result<ConditionSettings, Vec<Issue>> {
    let mut issues = Vec::new();
    let all = match setting(settings, "match") {
        "" | "all" => true,
        "any" => false,
        _ => {
            issues.push(issue("match", ISSUE_INVALID));
            true
        }
    };
    let indices = rule_indices(settings);
    if indices.is_empty() {
        issues.push(issue("rules", ISSUE_NO_RULES));
    }
    if indices.len() > MAX_RULES {
        issues.push(issue("rules", ISSUE_TOO_MANY_RULES));
    }
    let Some(subject) = subject else {
        issues.push(issue("rules", ISSUE_NO_SUBJECT));
        return Err(issues);
    };
    let mut rules = Vec::new();
    for index in indices {
        let key = |part: &str| format!("rules.{index}.{part}");
        let field = match setting(settings, &key("field")) {
            "" => {
                issues.push(issue(key("field"), ISSUE_MISSING));
                continue;
            }
            name => match subject.field(name) {
                Some(field) => field,
                None => {
                    issues.push(issue(key("field"), ISSUE_UNKNOWN_FIELD));
                    continue;
                }
            },
        };
        let Some(op) =
            Op::parse(setting(settings, &key("op"))).filter(|op| field.ty.ops().contains(op))
        else {
            issues.push(issue(key("op"), ISSUE_INVALID));
            continue;
        };
        let raw = setting(settings, &key("value"));
        let value = if !op.takes_value() {
            RuleValue::None
        } else if raw.is_empty() {
            issues.push(issue(key("value"), ISSUE_MISSING));
            continue;
        } else {
            match parse_value(field.ty, raw) {
                Some(value) => value,
                None => {
                    issues.push(issue(key("value"), ISSUE_INVALID));
                    continue;
                }
            }
        };
        rules.push(Rule { field, op, value });
    }
    if issues.is_empty() {
        Ok(ConditionSettings { all, rules })
    } else {
        Err(issues)
    }
}

/// What a condition compares with, read from the subject when the run gets
/// to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    Cents(u64),
    Number(u64),
    Text(String),
    Country(String),
    Bool(bool),
}

/// The facts of a run's subject, by field key.
pub type Facts = BTreeMap<&'static str, Fact>;

impl Rule {
    /// Whether the rule holds for `fact`; a fact the subject does not have
    /// holds nothing.
    pub fn holds(&self, fact: Option<&Fact>) -> bool {
        let Some(fact) = fact else {
            return false;
        };
        let compare = |left: u64, right: u64| match self.op {
            Op::Eq => left == right,
            Op::Ne => left != right,
            Op::Gt => left > right,
            Op::Ge => left >= right,
            Op::Lt => left < right,
            Op::Le => left <= right,
            _ => false,
        };
        match (fact, &self.value) {
            (Fact::Cents(left), RuleValue::Cents(right))
            | (Fact::Number(left), RuleValue::Number(right)) => compare(*left, *right),
            (Fact::Text(left), RuleValue::Text(right)) => {
                let left = left.to_lowercase();
                match self.op {
                    Op::Eq => left == *right,
                    Op::Ne => left != *right,
                    Op::Contains => left.contains(right.as_str()),
                    _ => false,
                }
            }
            (Fact::Country(left), RuleValue::Country(right)) => match self.op {
                Op::Eq => left.eq_ignore_ascii_case(right),
                Op::Ne => !left.eq_ignore_ascii_case(right),
                _ => false,
            },
            (Fact::Bool(value), RuleValue::None) => match self.op {
                Op::IsTrue => *value,
                Op::IsFalse => !*value,
                _ => false,
            },
            _ => false,
        }
    }
}

impl ConditionSettings {
    /// Whether the run goes down the Yes lane.
    pub fn holds(&self, facts: &Facts) -> bool {
        let mut results = self
            .rules
            .iter()
            .map(|rule| rule.holds(facts.get(rule.field.key)));
        if self.all {
            results.all(|holds| holds)
        } else {
            results.any(|holds| holds)
        }
    }
}

/// A notice to the team, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifySettings {
    pub subject: String,
    /// What the merchant wants the team to read; may be empty.
    pub note: String,
}

pub fn parse_notify(settings: &BTreeMap<String, String>) -> Result<NotifySettings, Vec<Issue>> {
    let mut issues = Vec::new();
    let subject = setting(settings, "subject");
    if subject.is_empty() {
        issues.push(issue("subject", ISSUE_MISSING));
    } else if subject.chars().count() > MAX_SUBJECT_LEN || subject.chars().any(char::is_control) {
        issues.push(issue("subject", ISSUE_INVALID));
    }
    let note = settings.get("note").map(|note| note.trim()).unwrap_or("");
    if note.chars().count() > MAX_NOTE_LEN {
        issues.push(issue("note", ISSUE_INVALID));
    }
    if issues.is_empty() {
        Ok(NotifySettings {
            subject: subject.to_owned(),
            note: note.to_owned(),
        })
    } else {
        Err(issues)
    }
}

/// The longest subject of a mail to a customer, in characters.
pub const MAX_EMAIL_SUBJECT_LEN: usize = 200;
/// The longest body of a mail to a customer (HTML or text), in bytes.
pub const MAX_EMAIL_BODY_LEN: usize = 100 * 1024;

/// A mail to the customer, read: templates over the shop's variables and
/// the subject's, which the host checks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailSettings {
    pub subject: String,
    pub html: String,
    /// The plain-text part; none derives it from the HTML.
    pub text: Option<String>,
}

pub fn parse_email(settings: &BTreeMap<String, String>) -> Result<EmailSettings, Vec<Issue>> {
    let mut issues = Vec::new();
    let subject = setting(settings, "subject");
    if subject.is_empty() {
        issues.push(issue("subject", ISSUE_MISSING));
    } else if subject.chars().count() > MAX_EMAIL_SUBJECT_LEN
        || subject.chars().any(char::is_control)
    {
        issues.push(issue("subject", ISSUE_INVALID));
    }
    let html = settings.get("html").map(|html| html.trim()).unwrap_or("");
    if html.is_empty() {
        issues.push(issue("html", ISSUE_MISSING));
    } else if html.len() > MAX_EMAIL_BODY_LEN {
        issues.push(issue("html", ISSUE_INVALID));
    }
    let text = settings
        .get("text")
        .map(|text| text.trim())
        .filter(|text| !text.is_empty());
    if text.is_some_and(|text| text.len() > MAX_EMAIL_BODY_LEN) {
        issues.push(issue("text", ISSUE_INVALID));
    }
    if issues.is_empty() {
        Ok(EmailSettings {
            subject: subject.to_owned(),
            html: html.to_owned(),
            text: text.map(str::to_owned),
        })
    } else {
        Err(issues)
    }
}

/// The longest prefix of a code a scenario makes, in characters.
pub const MAX_CODE_PREFIX_LEN: usize = 12;
/// The longest a code a scenario makes is valid, in days.
pub const MAX_CODE_DAYS: u32 = 365;

/// What a code a scenario makes takes off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodeValue {
    /// In basis points (1000 is 10 %).
    Percent(u32),
    Amount {
        cents: u64,
        currency: String,
    },
}

/// A discount code step, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscountSettings {
    pub value: CodeValue,
    /// How many days the code is valid from when it is made.
    pub days: u32,
    /// What the code starts with (`THANKS` gives `THANKS-7KQ2M9PX`).
    pub prefix: String,
}

pub fn parse_discount(settings: &BTreeMap<String, String>) -> Result<DiscountSettings, Vec<Issue>> {
    let mut issues = Vec::new();
    let value = match (
        setting(settings, "kind"),
        parse_cents(setting(settings, "value")),
    ) {
        ("percent", Some(hundredths)) if (1..=10_000).contains(&hundredths) => {
            Some(CodeValue::Percent(hundredths as u32))
        }
        ("amount", Some(cents)) if (1..=100_000_000).contains(&cents) => {
            let currency = setting(settings, "currency").to_ascii_uppercase();
            if currency.len() == 3 && currency.bytes().all(|b| b.is_ascii_uppercase()) {
                Some(CodeValue::Amount { cents, currency })
            } else {
                issues.push(issue("currency", ISSUE_INVALID));
                None
            }
        }
        ("percent" | "amount", None) if setting(settings, "value").is_empty() => {
            issues.push(issue("value", ISSUE_MISSING));
            None
        }
        ("percent" | "amount", _) => {
            issues.push(issue("value", ISSUE_INVALID));
            None
        }
        _ => {
            issues.push(issue("kind", ISSUE_INVALID));
            None
        }
    };
    let days = match setting(settings, "days").parse::<u32>() {
        Ok(days) if (1..=MAX_CODE_DAYS).contains(&days) => Some(days),
        _ => {
            issues.push(issue("days", ISSUE_INVALID));
            None
        }
    };
    let prefix = setting(settings, "prefix").to_ascii_uppercase();
    if prefix.is_empty()
        || prefix.len() > MAX_CODE_PREFIX_LEN
        || !prefix
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        issues.push(issue("prefix", ISSUE_INVALID));
    }
    match (value, days) {
        (Some(value), Some(days)) if issues.is_empty() => Ok(DiscountSettings {
            value,
            days,
            prefix,
        }),
        _ => Err(issues),
    }
}

/// A webhook step, read. The address is a secret (a hook's address often is
/// its only key) and the definition replicates to storefronts, so it is kept
/// sealed by the host (`url_sealed`); only its host is in clear, to show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSettings {
    pub url_sealed: String,
    pub host: String,
}

pub fn parse_webhook(settings: &BTreeMap<String, String>) -> Result<WebhookSettings, Vec<Issue>> {
    let url_sealed = setting(settings, "url_sealed");
    if url_sealed.is_empty() {
        return Err(vec![issue("url", ISSUE_MISSING)]);
    }
    Ok(WebhookSettings {
        url_sealed: url_sealed.to_owned(),
        host: setting(settings, "host").to_owned(),
    })
}

/// What a new step of `kind` starts with, for a run about `subject`.
pub fn default_settings(kind: &str, subject: Option<Subject>) -> BTreeMap<String, String> {
    let pairs: Vec<(String, String)> = match kind {
        WAIT => vec![
            ("amount".into(), "1".into()),
            ("unit".into(), Unit::Days.as_str().into()),
        ],
        DISCOUNT => vec![
            ("kind".into(), "percent".into()),
            ("value".into(), "10".into()),
            ("currency".into(), "EUR".into()),
            ("days".into(), "30".into()),
            ("prefix".into(), "THANKS".into()),
        ],
        CONDITION => {
            let mut pairs = vec![("match".to_owned(), "all".to_owned())];
            if let Some(field) = subject.and_then(|subject| subject.fields().first()) {
                pairs.push(("rules.0.field".into(), field.key.into()));
                pairs.push(("rules.0.op".into(), field.ty.ops()[0].as_str().into()));
                pairs.push(("rules.0.value".into(), String::new()));
            }
            pairs
        }
        _ => Vec::new(),
    };
    pairs.into_iter().collect()
}

/// The issues of a step's settings, by kind; none for a kind this build does
/// not know (the definition reports the kind itself).
pub fn step_issues(
    kind: &str,
    subject: Option<Subject>,
    settings: &BTreeMap<String, String>,
) -> Vec<Issue> {
    let result = match kind {
        WAIT => parse_wait(settings).map(|_| ()),
        CONDITION => parse_condition(subject, settings).map(|_| ()),
        NOTIFY => parse_notify(settings).map(|_| ()),
        EMAIL => parse_email(settings).map(|_| ()),
        DISCOUNT => parse_discount(settings).map(|_| ()),
        WEBHOOK => parse_webhook(settings).map(|_| ()),
        _ => Ok(()),
    };
    result.err().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
            .collect()
    }

    #[test]
    fn amounts_read_with_a_dot_or_a_comma() {
        assert_eq!(parse_cents("49.90"), Some(4990));
        assert_eq!(parse_cents("49,9"), Some(4990));
        assert_eq!(parse_cents("50"), Some(5000));
        assert_eq!(parse_cents("0.05"), Some(5));
        assert_eq!(parse_cents("1.234"), None);
        assert_eq!(parse_cents("-3"), None);
        assert_eq!(parse_cents(".5"), None);
        assert_eq!(parse_cents("abc"), None);
    }

    #[test]
    fn a_wait_reads_its_amount_and_unit() {
        let wait = parse_wait(&map(&[("amount", "2"), ("unit", "days")])).unwrap();
        assert_eq!(wait.secs(), 2 * 24 * 60 * 60);
        let issues = parse_wait(&map(&[("amount", "0"), ("unit", "weeks")])).unwrap_err();
        assert_eq!(issues.len(), 2);
        // Longer than the longest wait.
        assert!(parse_wait(&map(&[("amount", "91"), ("unit", "days")])).is_err());
    }

    #[test]
    fn a_condition_reads_its_rules_for_its_subject() {
        let settings = map(&[
            ("match", "any"),
            ("rules.0.field", "order.total"),
            ("rules.0.op", "ge"),
            ("rules.0.value", "50"),
            ("rules.1.field", "order.country"),
            ("rules.1.op", "eq"),
            ("rules.1.value", "fr"),
            ("rules.2.field", "order.first"),
            ("rules.2.op", "true"),
        ]);
        let condition = parse_condition(Some(Subject::Order), &settings).unwrap();
        assert!(!condition.all);
        assert_eq!(condition.rules.len(), 3);
        assert_eq!(condition.rules[0].value, RuleValue::Cents(5000));
        assert_eq!(condition.rules[1].value, RuleValue::Country("FR".into()));
        assert_eq!(condition.rules[2].value, RuleValue::None);

        // A review has no total.
        let issues = parse_condition(Some(Subject::Review), &settings).unwrap_err();
        assert!(issues.iter().any(|issue| issue.code == ISSUE_UNKNOWN_FIELD));
        // No trigger yet: nothing to check the rules against.
        let issues = parse_condition(None, &settings).unwrap_err();
        assert!(issues.iter().any(|issue| issue.code == ISSUE_NO_SUBJECT));
        // An operator the field's type does not take.
        let issues = parse_condition(
            Some(Subject::Order),
            &map(&[
                ("rules.0.field", "order.first"),
                ("rules.0.op", "ge"),
                ("rules.0.value", "1"),
            ]),
        )
        .unwrap_err();
        assert_eq!(issues[0].key, "rules.0.op");
        // No rule at all.
        let issues = parse_condition(Some(Subject::Order), &map(&[])).unwrap_err();
        assert_eq!(issues[0].code, ISSUE_NO_RULES);
    }

    #[test]
    fn a_new_condition_starts_on_the_subjects_first_field() {
        let settings = default_settings(CONDITION, Some(Subject::Review));
        assert_eq!(settings["rules.0.field"], "review.rating");
        assert_eq!(settings["rules.0.op"], "ge");
        // Its value is still to write.
        assert_eq!(
            step_issues(CONDITION, Some(Subject::Review), &settings),
            vec![Issue {
                key: "rules.0.value".into(),
                code: ISSUE_MISSING
            }]
        );
        assert!(step_issues(WAIT, None, &default_settings(WAIT, None)).is_empty());
    }

    #[test]
    fn a_condition_holds_on_the_subjects_facts() {
        let settings = map(&[
            ("rules.0.field", "order.total"),
            ("rules.0.op", "ge"),
            ("rules.0.value", "50"),
            ("rules.1.field", "order.country"),
            ("rules.1.op", "eq"),
            ("rules.1.value", "fr"),
        ]);
        let all = parse_condition(Some(Subject::Order), &settings).unwrap();
        let mut facts = Facts::new();
        facts.insert("order.total", Fact::Cents(6000));
        facts.insert("order.country", Fact::Country("FR".into()));
        assert!(all.holds(&facts));
        facts.insert("order.total", Fact::Cents(4999));
        assert!(!all.holds(&facts));
        let mut settings = settings;
        settings.insert("match".into(), "any".into());
        let any = parse_condition(Some(Subject::Order), &settings).unwrap();
        assert!(any.holds(&facts));
        // A fact the subject lacks holds nothing.
        facts.remove("order.country");
        assert!(!any.holds(&facts));

        let first = parse_condition(
            Some(Subject::Order),
            &map(&[("rules.0.field", "order.first"), ("rules.0.op", "false")]),
        )
        .unwrap();
        facts.insert("order.first", Fact::Bool(false));
        assert!(first.holds(&facts));
        let language = parse_condition(
            Some(Subject::Message),
            &map(&[
                ("rules.0.field", "message.language"),
                ("rules.0.op", "eq"),
                ("rules.0.value", "FR"),
            ]),
        )
        .unwrap();
        let mut facts = Facts::new();
        facts.insert("message.language", Fact::Text("fr".into()));
        assert!(language.holds(&facts));
    }

    #[test]
    fn a_code_step_reads_its_value_and_validity() {
        let read = parse_discount(&default_settings(DISCOUNT, None)).unwrap();
        assert_eq!(read.value, CodeValue::Percent(1000));
        assert_eq!((read.days, read.prefix.as_str()), (30, "THANKS"));
        let amount = parse_discount(&map(&[
            ("kind", "amount"),
            ("value", "5,50"),
            ("currency", "eur"),
            ("days", "7"),
            ("prefix", "back2"),
        ]))
        .unwrap();
        assert_eq!(
            amount.value,
            CodeValue::Amount {
                cents: 550,
                currency: "EUR".into()
            }
        );
        assert_eq!(amount.prefix, "BACK2");
        let issues = parse_discount(&map(&[
            ("kind", "percent"),
            ("value", "150"),
            ("days", "0"),
            ("prefix", "no way"),
        ]))
        .unwrap_err();
        assert_eq!(issues.len(), 3);
    }

    #[test]
    fn a_notice_needs_a_subject() {
        assert!(parse_notify(&map(&[("subject", "Big order"), ("note", "")])).is_ok());
        let issues = parse_notify(&map(&[("subject", " ")])).unwrap_err();
        assert_eq!(issues[0].code, ISSUE_MISSING);
    }
}
