//! The Discord step: a channel's webhook address of the merchant's, kept
//! sealed in the scenario as a webhook step's is, posted the merchant's
//! message and a link to what started the run.
//!
//! The message is a template over a few facts of the subject ([`vars_of`]),
//! none of them personal and none written by a shopper: the channel is
//! another system, and whatever a message mentions (`@here`, a role) must be
//! the merchant's choice.
//!
//! Discord takes no idempotency key: a call that got no answer may have been
//! posted, and trying it again may post it twice. A line said twice in a
//! channel is worth less trouble than a new order not said at all.

use serde_json::{Value, json};
use timada_automations::{
    Definition,
    catalog::{self, DISCORD, Subject},
};
use timada_orders::OrderState;
use timada_tenancy::{MailTemplateError, render_message};

use super::webhook::{Delivery, status_delivery};

/// The most Discord takes in one message, in characters.
const MAX_CONTENT_LEN: usize = 2000;

/// Whether `url` is a Discord channel's webhook address. Only these are
/// taken, so the step is no way to post anywhere else; `allow_private_hosts`
/// (development and tests) also takes a host of this machine, for a stand-in.
pub fn is_webhook_url(url: &reqwest::Url, allow_private_hosts: bool) -> bool {
    let official = url.scheme() == "https"
        && matches!(
            url.host_str(),
            Some("discord.com" | "ptb.discord.com" | "canary.discord.com" | "discordapp.com")
        );
    let stand_in = allow_private_hosts && url.host_str() == Some("127.0.0.1");
    (official || stand_in) && url.path().starts_with("/api/webhooks/")
}

/// What is posted: the message, then the link, in `<>` for Discord not to
/// unfold the dashboard's sign-in page under it. A template can print more
/// than Discord takes: the message gives way, not the link.
pub fn content(message: &str, link: &str) -> String {
    let room = MAX_CONTENT_LEN.saturating_sub(link.chars().count() + 3);
    let message: String = message.chars().take(room).collect();
    format!("{message}\n<{link}>")
}

fn order_vars(reference: &str, total: &str, items: u64, country: &str) -> Value {
    json!({ "reference": reference, "total": total, "items": items, "country": country })
}

fn of_order(order: &OrderState) -> Value {
    order_vars(
        &order.reference,
        &timada_core::format_money(order.total_cents, &order.currency),
        order
            .lines
            .iter()
            .map(|line| u64::from(line.quantity))
            .sum(),
        &order.shipping.country,
    )
}

/// What a message may print about a tenant's subject; `None` once the
/// subject is gone. Only what is listed here reaches Discord.
pub async fn vars_of<E: evento::Executor + Clone>(
    store: &E,
    tenant_id: &str,
    shop: &str,
    subject: Subject,
    subject_id: &str,
) -> anyhow::Result<Option<Value>> {
    let orders = timada_orders::Command::new(store.clone(), tenant_id);
    let mut vars = json!({ "shop": { "name": shop } });
    match subject {
        Subject::Order => {
            let Some(order) = orders.load_order(subject_id).await? else {
                return Ok(None);
            };
            vars["order"] = of_order(&order);
        }
        Subject::Review => {
            let reviews = timada_reviews::Command::new(store.clone(), tenant_id);
            let Some(review) = reviews.load_review(subject_id).await? else {
                return Ok(None);
            };
            let Some(order) = orders.load_order(&review.order_id).await? else {
                return Ok(None);
            };
            vars["review"] = json!({ "rating": review.rating });
            vars["order"] = of_order(&order);
        }
        Subject::Cart => {
            let Some(cart) = orders.load_cart(subject_id).await? else {
                return Ok(None);
            };
            let total = cart
                .requests
                .last()
                .and_then(|request| request.expected_total_cents)
                .map(timada_core::format_amount);
            let items: u64 = cart.lines.iter().map(|line| u64::from(line.quantity)).sum();
            vars["cart"] = json!({ "total": total, "items": items });
        }
        Subject::Shopper | Subject::Subscriber | Subject::Message => {}
    }
    Ok(Some(vars))
}

/// Made-up variables of the same shape, to check and preview a message.
fn sample(shop: &str, subject: Option<Subject>) -> Value {
    let mut vars = json!({ "shop": { "name": shop } });
    if matches!(subject, Some(Subject::Order | Subject::Review)) {
        vars["order"] = order_vars("7KQ2M9PX", &timada_core::format_money(4990, "EUR"), 2, "FR");
    }
    if subject == Some(Subject::Review) {
        vars["review"] = json!({ "rating": 5 });
    }
    if subject == Some(Subject::Cart) {
        vars["cart"] = json!({ "total": timada_core::format_amount(4990), "items": 2 });
    }
    vars
}

/// The variables a message about `subject` may print, for the editor to say.
pub fn variables(subject: Option<Subject>) -> &'static str {
    match subject {
        Some(Subject::Order) => {
            "{{ shop.name }}, {{ order.reference }}, {{ order.total }}, {{ order.items }}, {{ order.country }}"
        }
        Some(Subject::Review) => {
            "{{ shop.name }}, {{ review.rating }}, {{ order.reference }}, {{ order.total }}, {{ order.items }}, {{ order.country }}"
        }
        Some(Subject::Cart) => "{{ shop.name }}, {{ cart.total }}, {{ cart.items }}",
        _ => "{{ shop.name }}",
    }
}

/// A message rendered with made-up variables, or why it does not read.
pub fn preview(
    shop: &str,
    subject: Option<Subject>,
    message: &str,
) -> Result<String, MailTemplateError> {
    render_message(message.trim(), &sample(shop, subject))
}

/// Whether every Discord message of the scenario reads.
pub fn messages_read(shop: &str, definition: &Definition) -> bool {
    definition
        .walk()
        .iter()
        .filter(|step| step.kind == DISCORD)
        .all(|step| {
            catalog::parse_discord(&step.settings).is_ok_and(|settings| {
                preview(shop, definition.subject(), &settings.message).is_ok()
            })
        })
}

/// Posts `content` to the channel. A host that is not public is refused
/// unless `allow_private_hosts` (development and tests).
pub async fn post(
    client: &reqwest::Client,
    url: &str,
    content: &str,
    allow_private_hosts: bool,
) -> Delivery {
    if !allow_private_hosts && let Err(err) = crate::net::check_public_url(url).await {
        return Delivery::Refused(err.to_string());
    }
    let sent = client
        .post(url)
        .json(&serde_json::json!({ "content": content }))
        .send()
        .await;
    match sent {
        Ok(response) => status_delivery(response.status().as_u16()),
        // Not the error's words: they quote the address, which is the key.
        Err(_) => Delivery::Again("Discord could not be reached.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taken(url: &str, allow_private_hosts: bool) -> bool {
        is_webhook_url(&reqwest::Url::parse(url).unwrap(), allow_private_hosts)
    }

    #[test]
    fn only_a_discord_webhook_address_is_taken() {
        assert!(taken("https://discord.com/api/webhooks/1/abc", false));
        assert!(taken("https://discordapp.com/api/webhooks/1/abc", false));
        assert!(taken(
            "https://canary.discord.com/api/webhooks/1/abc",
            false
        ));
        // Not Discord, not a webhook, not encrypted, or only named like it.
        assert!(!taken("https://example.com/api/webhooks/1/abc", false));
        assert!(!taken("https://discord.com/channels/1/2", false));
        assert!(!taken("http://discord.com/api/webhooks/1/abc", false));
        assert!(!taken(
            "https://discord.com.example.org/api/webhooks/1/abc",
            false
        ));
        // A stand-in, in development and tests only.
        assert!(!taken("http://127.0.0.1:12118/api/webhooks/1/abc", false));
        assert!(taken("http://127.0.0.1:12118/api/webhooks/1/abc", true));
    }

    #[test]
    fn a_long_message_gives_way_to_the_link() {
        let link = "https://d.example/t/acme/orders/o1";
        let posted = content(&"x".repeat(5000), link);
        assert_eq!(posted.chars().count(), MAX_CONTENT_LEN);
        assert!(posted.ends_with(&format!("\n<{link}>")));
    }

    #[test]
    fn an_order_tells_nothing_personal() {
        let vars = of_order(&OrderState {
            reference: "ABC".into(),
            email: "ada@example.org".into(),
            total_cents: 4990,
            currency: "EUR".into(),
            shipping: timada_orders::ShippingAddress {
                name: "Ada Lovelace".into(),
                line1: "1 rue de la Paix".into(),
                city: "Paris".into(),
                country: "FR".into(),
                ..Default::default()
            },
            ..Default::default()
        });
        let printed = vars.to_string();
        for private in ["ada@", "Lovelace", "Paix", "Paris"] {
            assert!(!printed.contains(private), "{printed}");
        }
        assert_eq!(vars["reference"], "ABC");
        assert_eq!(vars["country"], "FR");
    }

    #[test]
    fn a_message_prints_what_its_subject_has() {
        assert_eq!(
            preview(
                "Acme",
                Some(Subject::Order),
                "{{ shop.name }}: {{ order.reference }}"
            )
            .unwrap(),
            "Acme: 7KQ2M9PX"
        );
        // A contact message has no order, and no one's name to print.
        assert!(preview("Acme", Some(Subject::Message), "{{ order.total }}").is_err());
        assert!(preview("Acme", Some(Subject::Order), "{{ order.email }}").is_err());
        assert!(preview("Acme", None, "{% if %}").is_err());
        // Every variable the editor lists reads.
        for subject in [
            Subject::Order,
            Subject::Review,
            Subject::Cart,
            Subject::Message,
        ] {
            for name in variables(Some(subject)).split(", ") {
                assert!(preview("Acme", Some(subject), name).is_ok(), "{name}");
            }
        }
    }

    #[test]
    fn the_link_goes_under_the_message_without_unfolding() {
        assert_eq!(
            content("New order", "https://d.example/t/acme/orders/o1"),
            "New order\n<https://d.example/t/acme/orders/o1>"
        );
    }
}
