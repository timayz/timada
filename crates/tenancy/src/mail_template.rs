//! The mails a shop sends its shoppers, as the merchant words them.
//!
//! A template is a subject and a whole HTML document in a small, sandboxed
//! template language (`{{ order.reference }}`, `{% for line in order.lines
//! %}`). Each mail has its documented set of variables; a template naming any
//! other is refused when saved. Variables are HTML-escaped, so what a shopper
//! typed (a note, an address) can never add markup, and the template itself
//! may not carry scripts, frames, forms or event handlers.
//!
//! A shop without its own template sends the built-in one, which words the
//! mails as they always were.

use std::cell::RefCell;
use std::collections::HashSet;

use lol_html::{RewriteStrSettings, element, rewrite_str};
use minijinja::value::Serde;
use minijinja::{AutoEscape, Environment, UndefinedBehavior, Value};
use serde::Serialize;

/// The longest subject a template may have, in characters.
pub const MAX_MAIL_SUBJECT_LEN: usize = 200;
/// The largest HTML a template may have, in bytes.
pub const MAX_MAIL_HTML_LEN: usize = 100 * 1024;
/// How much work one render may do before it is stopped, so a runaway
/// `{% for %}` can't hold a mail up.
const RENDER_FUEL: u64 = 200_000;
/// Functions the template language provides, which the variable check
/// must not mistake for unknown variables.
const GLOBALS: [&str; 4] = ["range", "dict", "namespace", "loop"];

/// One of the mails a shop sends its shoppers. Stored by [`key`](Self::key).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShopMailKind {
    ShopperCode,
    OrderConfirmation,
    ReviewRequest,
}

/// A variable a template may use, as the editor lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MailVariable {
    pub name: &'static str,
    pub description: &'static str,
}

const SHOP_VARIABLES: [MailVariable; 1] = [MailVariable {
    name: "shop.name",
    description: "The shop's name.",
}];

const SHOPPER_CODE_VARIABLES: [MailVariable; 2] = [
    MailVariable {
        name: "code",
        description: "The sign-in code.",
    },
    MailVariable {
        name: "valid_minutes",
        description: "How many minutes the code works for.",
    },
];

const ADDRESS_FIELDS: [&str; 6] = ["name", "line1", "line2", "postal_code", "city", "country"];

const ORDER_CONFIRMATION_VARIABLES: [MailVariable; 11] = [
    MailVariable {
        name: "order.reference",
        description: "The order's reference.",
    },
    MailVariable {
        name: "order.lines",
        description: "The lines, a list: {% for line in order.lines %} … {% endfor %}, \
                      each with line.name, line.variant (may be empty), line.quantity \
                      and line.amount.",
    },
    MailVariable {
        name: "order.subtotal",
        description: "The lines' total, before discount and shipping.",
    },
    MailVariable {
        name: "order.discount",
        description: "The discount, when a code was used: order.discount.code and \
                      order.discount.amount. Check it with {% if order.discount %}.",
    },
    MailVariable {
        name: "order.shipping_method",
        description: "The shipping method, when there is one: order.shipping_method.name, \
                      order.shipping_method.amount and order.shipping_method.free.",
    },
    MailVariable {
        name: "order.total",
        description: "What the shopper paid.",
    },
    MailVariable {
        name: "order.shipping_address",
        description: "Where the order goes: .name, .line1, .line2 (may be empty), \
                      .postal_code, .city and .country.",
    },
    MailVariable {
        name: "order.billing_address",
        description: "The billing address when it differs, with the same fields. \
                      Check it with {% if order.billing_address %}.",
    },
    MailVariable {
        name: "order.note",
        description: "The shopper's note, may be empty.",
    },
    MailVariable {
        name: "order.email",
        description: "The shopper's e-mail address.",
    },
    MailVariable {
        name: "link",
        description: "The order's page on the shop, when the shop has an address.",
    },
];

const REVIEW_REQUEST_VARIABLES: [MailVariable; 4] = [
    MailVariable {
        name: "order.reference",
        description: "The order's reference.",
    },
    MailVariable {
        name: "products",
        description: "The names of the products not reviewed yet, a list: \
                      {% for product in products %} … {% endfor %}.",
    },
    MailVariable {
        name: "link",
        description: "The page where the shopper reviews the order.",
    },
    MailVariable {
        name: "valid_days",
        description: "How many days the link works for.",
    },
];

impl ShopMailKind {
    pub const ALL: [Self; 3] = [
        Self::ShopperCode,
        Self::OrderConfirmation,
        Self::ReviewRequest,
    ];

    /// The stored name, frozen once shipped.
    pub fn key(self) -> &'static str {
        match self {
            Self::ShopperCode => "shopper_code",
            Self::OrderConfirmation => "order_confirmation",
            Self::ReviewRequest => "review_request",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ShopperCode => "Sign-in code",
            Self::OrderConfirmation => "Order confirmation",
            Self::ReviewRequest => "Review request",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::ShopperCode => "Sent when a shopper signs in to their account.",
            Self::OrderConfirmation => "Sent once an order is paid.",
            Self::ReviewRequest => "Sent after an order is fulfilled, asking for a review.",
        }
    }

    /// What the template may use: the shop's variables, then the mail's own.
    pub fn variables(self) -> Vec<MailVariable> {
        let own: &[MailVariable] = match self {
            Self::ShopperCode => &SHOPPER_CODE_VARIABLES,
            Self::OrderConfirmation => &ORDER_CONFIRMATION_VARIABLES,
            Self::ReviewRequest => &REVIEW_REQUEST_VARIABLES,
        };
        SHOP_VARIABLES.iter().chain(own).copied().collect()
    }

    /// The variables a link may point at whole (`href="{{ link }}"`).
    fn url_variables(self) -> &'static [&'static str] {
        match self {
            Self::ShopperCode => &[],
            Self::OrderConfirmation | Self::ReviewRequest => &["link"],
        }
    }

    /// Every path a template may look up, the nested fields included.
    fn allowed_paths(self) -> HashSet<String> {
        let mut paths: HashSet<String> = self
            .variables()
            .iter()
            .map(|variable| variable.name.to_owned())
            .collect();
        if self == Self::OrderConfirmation {
            for field in ["code", "amount"] {
                paths.insert(format!("order.discount.{field}"));
            }
            for field in ["name", "amount", "free"] {
                paths.insert(format!("order.shipping_method.{field}"));
            }
            for address in ["shipping_address", "billing_address"] {
                for field in ADDRESS_FIELDS {
                    paths.insert(format!("order.{address}.{field}"));
                }
            }
        }
        paths
    }

    pub fn default_subject(self) -> &'static str {
        match self {
            Self::ShopperCode => "Your sign-in code for {{ shop.name }}",
            Self::OrderConfirmation => "Your {{ shop.name }} order {{ order.reference }}",
            Self::ReviewRequest => "How was your {{ shop.name }} order {{ order.reference }}?",
        }
    }

    pub fn default_html(self) -> &'static str {
        match self {
            Self::ShopperCode => include_str!("mail_templates/shopper_code.html"),
            Self::OrderConfirmation => include_str!("mail_templates/order_confirmation.html"),
            Self::ReviewRequest => include_str!("mail_templates/review_request.html"),
        }
    }

    /// Made-up variables for the preview and the test mail, every optional
    /// part filled in.
    pub fn sample(self, shop_name: &str) -> MailVars {
        self.sample_with(shop_name, true)
    }

    fn sample_with(self, shop_name: &str, full: bool) -> MailVars {
        let shop = ShopVars {
            name: shop_name.to_owned(),
        };
        match self {
            Self::ShopperCode => MailVars::ShopperCode(ShopperCodeVars {
                shop,
                code: "428913".to_owned(),
                valid_minutes: 10,
            }),
            Self::OrderConfirmation => {
                let address = |name: &str| AddressVars {
                    name: name.to_owned(),
                    line1: "12 rue des Lilas".to_owned(),
                    line2: full.then(|| "Bâtiment B".to_owned()),
                    postal_code: "75011".to_owned(),
                    city: "Paris".to_owned(),
                    country: "FR".to_owned(),
                };
                MailVars::OrderConfirmation(Box::new(OrderConfirmationVars {
                    shop,
                    order: OrderVars {
                        reference: "A1B2C3".to_owned(),
                        email: "camille@example.com".to_owned(),
                        lines: vec![
                            LineVars {
                                name: "Linen shirt".to_owned(),
                                variant: full.then(|| "Blue, M".to_owned()),
                                quantity: 2,
                                amount: "90.00 EUR".to_owned(),
                            },
                            LineVars {
                                name: "Canvas tote".to_owned(),
                                variant: None,
                                quantity: 1,
                                amount: "25.00 EUR".to_owned(),
                            },
                        ],
                        subtotal: "115.00 EUR".to_owned(),
                        discount: full.then(|| DiscountVars {
                            code: "WELCOME10".to_owned(),
                            amount: "11.50 EUR".to_owned(),
                        }),
                        shipping_method: full.then(|| ShippingMethodVars {
                            name: "Colissimo".to_owned(),
                            amount: "4.90 EUR".to_owned(),
                            free: false,
                        }),
                        total: if full { "108.40 EUR" } else { "115.00 EUR" }.to_owned(),
                        shipping_address: address("Camille Martin"),
                        billing_address: full.then(|| address("Martin & Fils SARL")),
                        note: full.then(|| "Please leave it with the concierge.".to_owned()),
                    },
                    link: full.then(|| "https://shop.example.com/orders/A1B2C3".to_owned()),
                }))
            }
            Self::ReviewRequest => MailVars::ReviewRequest(ReviewRequestVars {
                shop,
                order: ReviewOrderVars {
                    reference: "A1B2C3".to_owned(),
                },
                products: vec!["Linen shirt".to_owned(), "Canvas tote".to_owned()],
                link: "https://shop.example.com/reviews/sample".to_owned(),
                valid_days: 60,
            }),
        }
    }
}

// =============================================================================
// Variables
// =============================================================================

/// The variables of one mail, as its template sees them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum MailVars {
    ShopperCode(ShopperCodeVars),
    OrderConfirmation(Box<OrderConfirmationVars>),
    ReviewRequest(ReviewRequestVars),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShopVars {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShopperCodeVars {
    pub shop: ShopVars,
    pub code: String,
    pub valid_minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderConfirmationVars {
    pub shop: ShopVars,
    pub order: OrderVars,
    pub link: Option<String>,
}

/// An order, its amounts already worded with their currency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrderVars {
    pub reference: String,
    pub email: String,
    pub lines: Vec<LineVars>,
    pub subtotal: String,
    pub discount: Option<DiscountVars>,
    pub shipping_method: Option<ShippingMethodVars>,
    pub total: String,
    pub shipping_address: AddressVars,
    pub billing_address: Option<AddressVars>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LineVars {
    pub name: String,
    pub variant: Option<String>,
    pub quantity: u32,
    pub amount: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiscountVars {
    pub code: String,
    /// What it took off, without a sign.
    pub amount: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShippingMethodVars {
    pub name: String,
    pub amount: String,
    pub free: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AddressVars {
    pub name: String,
    pub line1: String,
    pub line2: Option<String>,
    pub postal_code: String,
    pub city: String,
    pub country: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewRequestVars {
    pub shop: ShopVars,
    pub order: ReviewOrderVars,
    pub products: Vec<String>,
    pub link: String,
    pub valid_days: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReviewOrderVars {
    pub reference: String,
}

// =============================================================================
// Checking and rendering
// =============================================================================

/// Why a template was refused or could not be rendered, worded for the
/// merchant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct MailTemplateError(pub String);

/// A mail ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedMail {
    pub subject: String,
    pub html: String,
}

/// The sandbox templates render in: no other templates to include, no
/// `safe` filter to turn escaping off, `none` printed as nothing, and a
/// bounded amount of work.
fn environment() -> Environment<'static> {
    let mut env = Environment::new();
    env.remove_filter("safe");
    env.remove_global("debug");
    env.set_undefined_behavior(UndefinedBehavior::SemiStrict);
    env.set_fuel(Some(RENDER_FUEL));
    env.set_auto_escape_callback(|name| match name {
        "body.html" => AutoEscape::Html,
        _ => AutoEscape::None,
    });
    env.set_formatter(|out, state, value| {
        if value.is_none() {
            return Ok(());
        }
        minijinja::escape_formatter(out, state, value)
    });
    env
}

/// What the merchant reads for a template error: the part, the line and
/// what went wrong.
fn describe(part: &str, err: &minijinja::Error) -> MailTemplateError {
    let what = err
        .detail()
        .map_or_else(|| err.kind().to_string(), str::to_owned);
    MailTemplateError(match err.line() {
        Some(line) => format!("{part}, line {line}: {what}"),
        None => format!("{part}: {what}"),
    })
}

/// Renders `subject` and `html` with `vars`. The subject is plain text on
/// one line; the HTML has every variable escaped.
pub fn render(
    subject: &str,
    html: &str,
    vars: &MailVars,
) -> Result<RenderedMail, MailTemplateError> {
    let env = environment();
    let ctx = Value::from(Serde(vars));
    let subject = env
        .template_from_named_str("subject.txt", subject)
        .and_then(|template| template.render(&ctx))
        .map_err(|err| describe("Subject", &err))?;
    let html = env
        .template_from_named_str("body.html", html)
        .and_then(|template| template.render(&ctx))
        .map_err(|err| describe("Body", &err))?;
    Ok(RenderedMail {
        subject: subject.split_whitespace().collect::<Vec<_>>().join(" "),
        html,
    })
}

/// Whether a link or picture address is one a mail may carry: a web or
/// mail address written out, an anchor, a picture's bytes, or a whole URL
/// variable of the mail.
fn safe_url(kind: ShopMailKind, attribute: &str, value: &str) -> bool {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if ["https://", "http://", "mailto:", "tel:", "#"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
    {
        return true;
    }
    if attribute == "src"
        && lower.starts_with("data:image/")
        && !lower.starts_with("data:image/svg")
    {
        return true;
    }
    value
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
        .is_some_and(|name| kind.url_variables().contains(&name.trim()))
}

/// Refuses markup a mail must not carry: scripts, frames, plug-ins, forms,
/// event handlers and links that run code.
fn check_markup(kind: ShopMailKind, html: &str) -> Result<(), MailTemplateError> {
    const FORBIDDEN: [&str; 10] = [
        "script", "iframe", "frame", "frameset", "object", "embed", "applet", "form", "base",
        "link",
    ];
    let problem: RefCell<Option<String>> = RefCell::new(None);
    let settings = RewriteStrSettings::new().append_element_content_handler(element!("*", |el| {
        if problem.borrow().is_some() {
            return Ok(());
        }
        let tag = el.tag_name().to_ascii_lowercase();
        let found = if FORBIDDEN.contains(&tag.as_str()) {
            Some(format!("a mail may not contain <{tag}>"))
        } else if tag == "meta" && el.has_attribute("http-equiv") {
            Some("a mail may not contain <meta http-equiv>".to_owned())
        } else {
            el.attributes().iter().find_map(|attribute| {
                let name = attribute.name().to_ascii_lowercase();
                if name.starts_with("on") {
                    Some(format!("<{tag}> may not have an {name} attribute"))
                } else if matches!(
                    name.as_str(),
                    "href" | "src" | "action" | "formaction" | "background" | "poster"
                ) && !safe_url(kind, &name, &attribute.value())
                {
                    Some(format!(
                        "<{tag} {name}=\"…\"> must start with https:// (or http://, mailto:, \
                         tel:, #){}",
                        match kind.url_variables() {
                            [] => String::new(),
                            names => format!(" or be {{{{ {} }}}}", names.join(" }} or {{ ")),
                        }
                    ))
                } else {
                    None
                }
            })
        };
        *problem.borrow_mut() = found;
        Ok(())
    }));
    rewrite_str(html, settings)
        .map_err(|err| MailTemplateError(format!("Body: the HTML could not be read: {err}")))?;
    match problem.into_inner() {
        Some(problem) => Err(MailTemplateError(format!("Body: {problem}."))),
        None => Ok(()),
    }
}

/// Refuses variables the mail does not have.
fn check_variables(
    kind: ShopMailKind,
    env: &Environment<'_>,
    part: &str,
    name: &'static str,
    source: &str,
) -> Result<(), MailTemplateError> {
    let template = env
        .template_from_named_str(name, source)
        .map_err(|err| describe(part, &err))?;
    let allowed = kind.allowed_paths();
    let mut unknown: Vec<String> = template
        .undeclared_variables(true)
        .into_iter()
        .filter(|path| {
            let root = path.split('.').next().unwrap_or_default();
            !GLOBALS.contains(&root)
                && !allowed.contains(path)
                && !allowed.iter().any(|known| {
                    known.starts_with(path.as_str()) && known[path.len()..].starts_with('.')
                })
        })
        .collect();
    unknown.sort();
    match unknown.as_slice() {
        [] => Ok(()),
        [one] => Err(MailTemplateError(format!(
            "{part}: this mail has no variable {{{{ {one} }}}}."
        ))),
        many => Err(MailTemplateError(format!(
            "{part}: this mail has no variables {}.",
            many.iter()
                .map(|path| format!("{{{{ {path} }}}}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Checks a template before it is saved: it is within size, it reads, it
/// uses only the mail's variables, its markup is safe, and it renders both
/// with every optional part present and with none of them.
pub fn validate(kind: ShopMailKind, subject: &str, html: &str) -> Result<(), MailTemplateError> {
    if subject.trim().is_empty() {
        return Err(MailTemplateError(
            "Subject: a mail needs a subject.".to_owned(),
        ));
    }
    if subject.chars().count() > MAX_MAIL_SUBJECT_LEN {
        return Err(MailTemplateError(format!(
            "Subject: at most {MAX_MAIL_SUBJECT_LEN} characters."
        )));
    }
    if html.trim().is_empty() {
        return Err(MailTemplateError("Body: a mail needs a body.".to_owned()));
    }
    if html.len() > MAX_MAIL_HTML_LEN {
        return Err(MailTemplateError(format!(
            "Body: at most {} KB.",
            MAX_MAIL_HTML_LEN / 1024
        )));
    }
    let env = environment();
    check_variables(kind, &env, "Subject", "subject.txt", subject)?;
    check_variables(kind, &env, "Body", "body.html", html)?;
    check_markup(kind, html)?;
    render(subject, html, &kind.sample_with("Sample shop", true))?;
    render(subject, html, &kind.sample_with("Sample shop", false)).map_err(|err| {
        MailTemplateError(format!(
            "{err} (with the optional parts empty: check them with {{% if … %}})"
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_default_validates_and_renders() {
        for kind in ShopMailKind::ALL {
            validate(kind, kind.default_subject(), kind.default_html())
                .unwrap_or_else(|err| panic!("{kind:?}: {err}"));
            assert_eq!(ShopMailKind::from_key(kind.key()), Some(kind));
        }
    }

    #[test]
    fn the_default_order_confirmation_reads_as_before() {
        let kind = ShopMailKind::OrderConfirmation;
        let mail = render(
            kind.default_subject(),
            kind.default_html(),
            &kind.sample("Chez Léa"),
        )
        .unwrap();
        assert_eq!(mail.subject, "Your Chez Léa order A1B2C3");
        for text in [
            "Thank you for your order at <strong>Chez Léa</strong>.",
            "Linen shirt · Blue, M",
            "× 2",
            "Discount WELCOME10",
            "-11.50 EUR",
            "Shipping: Colissimo",
            "108.40 EUR",
            "Bâtiment B",
            "Martin &amp; Fils SARL",
            "Your note: Please leave it with the concierge.",
            "href=\"https:&#x2f;&#x2f;shop.example.com&#x2f;orders&#x2f;A1B2C3\"",
        ] {
            assert!(
                mail.html.contains(text),
                "missing {text:?} in {}",
                mail.html
            );
        }
        assert!(!mail.html.contains("none"));
    }

    #[test]
    fn optional_parts_left_out_print_nothing() {
        let kind = ShopMailKind::OrderConfirmation;
        let mail = render(
            kind.default_subject(),
            kind.default_html(),
            &kind.sample_with("Shop", false),
        )
        .unwrap();
        for text in [
            "Discount",
            "Billing to",
            "Your note",
            "View your order",
            "none",
        ] {
            assert!(!mail.html.contains(text), "unexpected {text:?}");
        }
    }

    #[test]
    fn variables_are_escaped() {
        let kind = ShopMailKind::ShopperCode;
        let mail = render(
            "Code for {{ shop.name }}",
            "<p>{{ shop.name }}</p>",
            &kind.sample("<b>Bold</b> & co"),
        )
        .unwrap();
        assert_eq!(mail.html, "<p>&lt;b&gt;Bold&lt;&#x2f;b&gt; &amp; co</p>");
        assert_eq!(mail.subject, "Code for <b>Bold</b> & co");
    }

    #[test]
    fn escaping_cannot_be_turned_off() {
        let err = validate(
            ShopMailKind::ShopperCode,
            "Code",
            "<p>{{ shop.name | safe }}</p>",
        )
        .unwrap_err();
        assert!(err.0.contains("safe"), "{err}");
    }

    #[test]
    fn unknown_variables_are_refused() {
        let err = validate(
            ShopMailKind::ShopperCode,
            "Code",
            "<p>{{ order.reference }}</p>",
        )
        .unwrap_err();
        assert_eq!(
            err.0,
            "Body: this mail has no variable {{ order.reference }}."
        );
        let err =
            validate(ShopMailKind::ShopperCode, "{{ nope }}", "<p>{{ code }}</p>").unwrap_err();
        assert_eq!(err.0, "Subject: this mail has no variable {{ nope }}.");
    }

    #[test]
    fn nested_and_loop_variables_are_known() {
        validate(
            ShopMailKind::OrderConfirmation,
            "Order {{ order.reference }}",
            "{% for line in order.lines %}{{ loop.index }}. {{ line.name }}{% endfor %}\
             {% if order.billing_address %}{{ order.billing_address.city }}{% endif %}",
        )
        .unwrap();
    }

    #[test]
    fn a_broken_block_is_refused_with_its_line() {
        let err = validate(
            ShopMailKind::ReviewRequest,
            "Review",
            "<ul>\n{% for product in products %}\n<li>{{ product }}</li>\n</ul>",
        )
        .unwrap_err();
        assert!(err.0.starts_with("Body, line"), "{err}");
    }

    #[test]
    fn a_missing_optional_check_is_refused() {
        let err = validate(
            ShopMailKind::OrderConfirmation,
            "Order",
            "<p>{{ order.discount.code }}</p>",
        )
        .unwrap_err();
        assert!(err.0.contains("optional parts empty"), "{err}");
    }

    #[test]
    fn unsafe_markup_is_refused() {
        for (html, expected) in [
            ("<script>alert(1)</script>", "<script>"),
            ("<p onclick=\"x()\">hi</p>", "onclick"),
            ("<a href=\"javascript:alert(1)\">x</a>", "href"),
            ("<img src=\"data:image/svg+xml,…\">", "src"),
            ("<a href=\"{{ shop.name }}\">x</a>", "href"),
            ("<iframe src=\"https://example.com\"></iframe>", "<iframe>"),
            ("<form><input></form>", "<form>"),
            ("<meta http-equiv=\"refresh\" content=\"0\">", "http-equiv"),
        ] {
            let err = validate(ShopMailKind::ReviewRequest, "Review", html).unwrap_err();
            assert!(err.0.contains(expected), "{html}: {err}");
        }
        validate(
            ShopMailKind::ReviewRequest,
            "Review",
            "<a href=\"{{ link }}\">Review</a> <a href=\"https://shop.example.com\">Shop</a>\
             <img src=\"https://shop.example.com/logo.png\" alt=\"\">",
        )
        .unwrap();
    }

    #[test]
    fn subject_and_size_limits() {
        let kind = ShopMailKind::ShopperCode;
        assert!(validate(kind, " ", "<p>x</p>").is_err());
        assert!(validate(kind, &"x".repeat(MAX_MAIL_SUBJECT_LEN + 1), "<p>x</p>").is_err());
        assert!(validate(kind, "x", &"x".repeat(MAX_MAIL_HTML_LEN + 1)).is_err());
    }

    #[test]
    fn a_runaway_loop_is_stopped() {
        let err = render(
            "x",
            "{% for i in range(100000) %}{% for j in range(100000) %}.{% endfor %}{% endfor %}",
            &ShopMailKind::ShopperCode.sample("Shop"),
        )
        .unwrap_err();
        assert!(err.0.starts_with("Body"), "{err}");
    }
}
