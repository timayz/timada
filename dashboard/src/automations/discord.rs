//! The Discord step: a channel's webhook address of the merchant's, kept
//! sealed in the scenario as a webhook step's is, posted the merchant's
//! message and a link to what started the run. Nothing a shopper wrote
//! travels, so what the message mentions is the merchant's choice.
//!
//! Discord takes no idempotency key: a call that got no answer may have been
//! posted, and trying it again may post it twice. A line said twice in a
//! channel is worth less trouble than a new order not said at all.

use super::webhook::{Delivery, status_delivery};

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
/// unfold the dashboard's sign-in page under it.
pub fn content(message: &str, link: &str) -> String {
    format!("{message}\n<{link}>")
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
    fn the_link_goes_under_the_message_without_unfolding() {
        assert_eq!(
            content("New order", "https://d.example/t/acme/orders/o1"),
            "New order\n<https://d.example/t/acme/orders/o1>"
        );
    }
}
