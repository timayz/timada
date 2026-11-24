//! Claude as the [`TextWriter`]: Anthropic's Messages API, under the
//! platform's own key (`[writer]` in the configuration).
//!
//! One call per draft, `POST /v1/messages`. The answer is asked for as
//! JSON of a fixed shape (`output_config.format`), so what comes back is
//! the four texts and nothing to pick apart. The model thinks before it
//! writes; its thinking is not asked for and is paid as output.
//!
//! The call takes at most [`TIMEOUT`]. What it cost is reckoned from the
//! tokens the answer counts, at the prices the configuration gives for the
//! model: Anthropic's answer names tokens, not dollars.
//!
//! A request the model's safeguards decline is run again by Anthropic on
//! another of its models (`fallbacks`), where the model takes that
//! parameter; such an answer is priced here as the configured model's,
//! which is close enough for a cap.

use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::{Answer, Ask, Failed, TextWriter, Texts, WriterFault};

/// Anthropic's API.
pub const API_URL: &str = "https://api.anthropic.com";

/// The version of the API the requests are written for.
const API_VERSION: &str = "2026-10-21";

/// What turns the fallback on a declined request on, and the models that
/// take it.
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
const FALLBACK_MODELS: [&str; 4] = [
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-fable-5-1",
    "claude-sonnet-5-5",
];

/// How long one call may take, connection and answer together: a few
/// hundred words after some thinking come back in well under a minute.
pub const TIMEOUT: Duration = Duration::from_secs(120);

/// The most tokens an answer may take, thinking included. Far above what
/// four short texts need, so an answer is not cut short.
const MAX_TOKENS: u64 = 16_000;

/// The most tokens a request is reckoned at for the room a cap keeps: the
/// instructions, and the longest product the catalog holds (a description
/// of 4,000 characters, thirty facts of 440).
const MOST_INPUT_TOKENS: u64 = 16_000;

/// How hard the model thinks: enough for careful wording, no more.
const EFFORT: &str = "medium";

const INSTRUCTIONS: &str = "\
You write the texts of an online shop's catalog: the name of a product or a category, its \
description, and the title and the description search engines show for its page. You write for \
the shopper who typed the search the request names, in the language the request names.

The request gives everything the shop knows of the subject. Use only that. Never state a size, a \
weight, a material, a power, a compatibility, a certification, a warranty or a delivery time the \
request does not give, however likely it seems: the shop answers for every word, and a person \
checks your draft against the same data. When a fact you would want is missing, write without it.

What the subject says now is raw material, often a supplier's machine translation: take its \
facts, not its wording. Do not copy its sentences.

Write as the shop asks when it gives a tone, and never write what it says never to claim. Without \
a tone: plain, concrete, in the second person where the language has a polite one, no \
superlatives the facts do not back, no exclamation marks, no prices, no promotions, no mention \
of other shops.

The search the texts are written for belongs in the name and in the search engines' title, as \
people type it, once. The description opens on what the shopper gains, then gives the facts. The \
search engines' description is one or two sentences that make the click worth it.

Plain text only: no Markdown, no HTML, no emoji. Paragraphs of the description are separated by \
a blank line. Each text stays within the number of characters the request gives for it. A text \
the request does not ask for is returned empty.";

/// What a million tokens cost, in US dollars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prices {
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

impl Prices {
    /// What `input` and `output` tokens cost, in millionths of a US
    /// dollar, rounded up: a million tokens at one dollar are a million
    /// millionths, so a token costs its price in millionths.
    fn cost(self, input: f64, output: f64) -> u64 {
        (input * self.input_usd_per_mtok + output * self.output_usd_per_mtok).ceil() as u64
    }
}

pub struct Claude {
    url: String,
    key: String,
    model: String,
    prices: Prices,
    client: reqwest::Client,
}

impl Claude {
    /// `url`: Anthropic's own ([`API_URL`]) when `None`.
    pub fn new(key: &str, url: Option<String>, model: &str, prices: Prices) -> Self {
        Self {
            url: url
                .unwrap_or_else(|| API_URL.to_owned())
                .trim_end_matches('/')
                .to_owned(),
            key: key.to_owned(),
            model: model.to_owned(),
            prices,
            client: reqwest::Client::builder()
                .timeout(TIMEOUT)
                .build()
                .unwrap_or_default(),
        }
    }

    fn falls_back(&self) -> bool {
        FALLBACK_MODELS.contains(&self.model.as_str())
    }

    /// The body of the request for `ask`.
    fn request(&self, ask: &Ask) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": INSTRUCTIONS,
            "messages": [{ "role": "user", "content": brief(ask) }],
            "output_config": {
                "effort": EFFORT,
                "format": {
                    "type": "json_schema",
                    "schema": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "meta_title": { "type": "string" },
                            "meta_description": { "type": "string" },
                        },
                        "required": ["name", "description", "meta_title", "meta_description"],
                        "additionalProperties": false,
                    },
                },
            },
        });
        if self.falls_back() {
            body["fallbacks"] = json!("default");
        }
        body
    }
}

/// The request as the model reads it: what to write, for which search, in
/// which language, from which facts.
fn brief(ask: &Ask) -> String {
    let mut lines: Vec<String> = Vec::new();
    let subject = if ask.product { "product" } else { "category" };
    lines.push(format!("Write the texts of this {subject}."));
    if ask.language.is_empty() {
        lines.push("Language: the one the name is written in now".to_owned());
    } else {
        lines.push(format!("Language: {}", ask.language));
    }
    lines.push(format!("Search to write for: {}", ask.keyword));
    lines.push(String::new());
    lines.push("Texts asked for, with the most characters each may be:".to_owned());
    if ask.product {
        lines.push(format!("- name: {}", ask.limits.name));
    } else {
        lines.push("- name: not asked for, return it empty".to_owned());
    }
    lines.push(format!("- description: {}", ask.limits.description));
    lines.push(format!("- meta_title: {}", ask.limits.meta_title));
    lines.push(format!(
        "- meta_description: {}",
        ask.limits.meta_description
    ));
    lines.push(String::new());
    lines.push(format!("What the {subject} says now:"));
    lines.push(format!("Name: {}", ask.now.name));
    if !ask.now.description.is_empty() {
        lines.push(format!("Description:\n{}", ask.now.description));
    }
    if !ask.facts.is_empty() {
        lines.push(String::new());
        lines.push("Facts:".to_owned());
        lines.extend(
            ask.facts
                .iter()
                .map(|(name, value)| format!("- {name}: {value}")),
        );
    }
    if !ask.holds.is_empty() {
        lines.push(String::new());
        lines.push("What it holds:".to_owned());
        lines.extend(ask.holds.iter().map(|name| format!("- {name}")));
    }
    if !ask.tone.is_empty() {
        lines.push(String::new());
        lines.push(format!("The shop's tone:\n{}", ask.tone));
    }
    if !ask.never.is_empty() {
        lines.push(String::new());
        lines.push(format!("Never claim or say:\n{}", ask.never));
    }
    lines.join("\n")
}

#[derive(Deserialize)]
struct Message {
    #[serde(default)]
    content: Vec<Block>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Usage,
}

#[derive(Deserialize)]
struct Block {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
}

/// The tokens an answer counts. Cached input is counted apart: written at
/// a quarter more than plain input, read at a tenth.
#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Deserialize)]
struct Written {
    name: String,
    description: String,
    meta_title: String,
    meta_description: String,
}

#[derive(Deserialize)]
struct Failure {
    error: FailureDetail,
}

#[derive(Deserialize)]
struct FailureDetail {
    #[serde(default)]
    message: String,
}

/// What an answer that is not a message means: asked too often, or the
/// API's own failure, may pass; anything else is the request's or the
/// account's and will not.
fn failure(status: u16, body: &str) -> WriterFault {
    let said = serde_json::from_str::<Failure>(body)
        .map(|failure| failure.error.message)
        .ok()
        .filter(|message| !message.is_empty())
        .unwrap_or_else(|| format!("HTTP {status}"));
    match status {
        408 | 409 | 429 | 500..=599 => WriterFault::Unavailable(said),
        _ => WriterFault::Refused(said),
    }
}

#[async_trait::async_trait]
impl TextWriter for Claude {
    async fn write(&self, ask: &Ask) -> Result<Answer, Failed> {
        let mut request = self
            .client
            .post(format!("{}/v1/messages", self.url))
            .header("x-api-key", &self.key)
            .header("anthropic-version", API_VERSION)
            .json(&self.request(ask));
        if self.falls_back() {
            request = request.header("anthropic-beta", FALLBACK_BETA);
        }
        let response = request
            .send()
            .await
            // The error names the address, never the key.
            .map_err(|err| WriterFault::Unavailable(err.without_url().to_string()))?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|err| WriterFault::Unavailable(err.without_url().to_string()))?;
        if !(200..300).contains(&status) {
            return Err(failure(status, &body).into());
        }
        let message: Message = serde_json::from_str(&body).map_err(|err| {
            WriterFault::Unavailable(format!("an answer that cannot be read: {err}"))
        })?;
        let usage = &message.usage;
        let cost_usd_micros = self.prices.cost(
            usage.input_tokens as f64
                + usage.cache_creation_input_tokens as f64 * 1.25
                + usage.cache_read_input_tokens as f64 * 0.1,
            usage.output_tokens as f64,
        );
        // What was paid counts even when nothing usable came back.
        let paid = |fault: fn(String) -> WriterFault, reason: &str| Failed {
            fault: fault(reason.to_owned()),
            cost_usd_micros,
        };
        match message.stop_reason.as_deref() {
            Some("refusal") => {
                return Err(paid(
                    WriterFault::Refused,
                    "the model declined to write this",
                ));
            }
            Some("max_tokens") => {
                return Err(paid(WriterFault::Unavailable, "the answer was cut short"));
            }
            _ => {}
        }
        let Some(text) = message.content.iter().find(|block| block.kind == "text") else {
            return Err(paid(WriterFault::Unavailable, "an answer without text"));
        };
        let written: Written = serde_json::from_str(&text.text).map_err(|_| {
            paid(
                WriterFault::Unavailable,
                "an answer that is not the texts asked for",
            )
        })?;
        Ok(Answer {
            texts: Texts {
                name: written.name,
                description: written.description,
                meta_title: written.meta_title,
                meta_description: written.meta_description,
            },
            cost_usd_micros,
        })
    }

    fn most_usd_micros(&self) -> u64 {
        self.prices
            .cost(MOST_INPUT_TOKENS as f64, MAX_TOKENS as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::super::Limits;
    use super::*;

    fn ask() -> Ask {
        Ask {
            product: true,
            language: "fr".to_owned(),
            keyword: "gants moto été".to_owned(),
            now: Texts {
                name: "Gants Moto Homme Respirant".to_owned(),
                description: "Gants de moto pour l'été.".to_owned(),
                ..Texts::default()
            },
            facts: vec![("Matière".to_owned(), "Cuir".to_owned())],
            holds: Vec::new(),
            tone: "Direct.".to_owned(),
            never: "Jamais « le meilleur ».".to_owned(),
            limits: Limits {
                name: 120,
                description: 4000,
                meta_title: 70,
                meta_description: 160,
            },
        }
    }

    const PRICES: Prices = Prices {
        input_usd_per_mtok: 4.0,
        output_usd_per_mtok: 20.0,
    };

    #[test]
    fn a_request_asks_for_the_four_texts_from_the_facts_given() {
        let claude = Claude::new("sk-test", None, "claude-opus-5-5", PRICES);
        let body = claude.request(&ask());
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["output_config"]["format"]["type"], "json_schema");
        assert_eq!(
            body["output_config"]["format"]["schema"]["additionalProperties"],
            false
        );
        let said = body["messages"][0]["content"].as_str().unwrap();
        for part in [
            "Write the texts of this product.",
            "Language: fr",
            "Search to write for: gants moto été",
            "- name: 120",
            "- meta_title: 70",
            "- Matière: Cuir",
            "The shop's tone:\nDirect.",
            "Never claim or say:\nJamais « le meilleur ».",
        ] {
            assert!(said.contains(part), "{part}: {said}");
        }
        // A category's name is not the writer's to change, and a model
        // that does not take the fallback is not sent it.
        let other = Claude::new("sk-test", None, "claude-haiku-5-5", PRICES);
        let mut category = ask();
        category.product = false;
        category.holds = vec!["Gants été".to_owned()];
        let body = other.request(&category);
        assert!(body.get("fallbacks").is_none());
        let said = body["messages"][0]["content"].as_str().unwrap();
        assert!(said.contains("- name: not asked for"), "{said}");
        assert!(said.contains("What it holds:\n- Gants été"), "{said}");
    }

    #[test]
    fn a_call_costs_its_tokens_at_the_models_prices() {
        // 1,500 tokens in and 900 out: 0.006 + 0.018 USD.
        assert_eq!(PRICES.cost(1_500.0, 900.0), 24_000);
        let claude = Claude::new("sk-test", None, "claude-opus-5-5", PRICES);
        // The room kept for a call: 16,000 tokens each way, 0.384 USD.
        assert_eq!(claude.most_usd_micros(), 384_000);
    }

    #[test]
    fn a_failure_says_whether_asking_again_may_do() {
        let overloaded =
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert_eq!(
            failure(529, overloaded),
            WriterFault::Unavailable("Overloaded".to_owned())
        );
        assert_eq!(
            failure(429, "{}"),
            WriterFault::Unavailable("HTTP 429".to_owned())
        );
        let key = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(
            failure(401, key),
            WriterFault::Refused("invalid x-api-key".to_owned())
        );
        assert_eq!(
            failure(400, "nope"),
            WriterFault::Refused("HTTP 400".to_owned())
        );
    }
}
