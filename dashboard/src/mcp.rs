//! The dashboard's MCP server: what an assistant reads of a shop and what
//! it may prepare, over HTTP, for whoever presents an agent key.
//!
//! One address takes JSON-RPC 2.0 requests (`POST /mcp`), one a request,
//! answered in the response's own body: `initialize`, `ping`,
//! `tools/list` and `tools/call`. A notification is taken and answered
//! with nothing. No session is kept and nothing is streamed: every call
//! stands by itself, on whichever node takes it.
//!
//! A key (`timada-tenancy`'s agent keys) is made by a user on their
//! account page, for the shops and permissions they choose. A call acts as
//! that user, within what the key covers and what the user still holds in
//! the shop when the call comes: a member who lost a permission, or left
//! the shop, takes the key's reach with them.
//!
//! The reading tools answer from what the pages read. The writing tools
//! each end in a proposal a person approves ([`crate::proposals`]),
//! prepared under the user's name with the key as its source: nothing an
//! agent asks for is published, priced or sent anywhere by itself. What
//! stays out altogether: approving, publishing, refunds, payments and
//! secrets.
//!
//! Every call is logged on the key's stream of the day, which its user
//! reads, and counted against a limit a key ([`CALLS_A_MINUTE`]).

mod tools;

use std::time::Duration;

use serde_json::{Value, json};
use timada_tenancy::{AgentKeyState, TenantState};

use crate::cluster::Executor;
use crate::db::Db;
use crate::rate_limit::RateLimiter;

pub use tools::TOOLS;

/// The revision of the protocol the server speaks when the client names
/// none it knows.
pub const PROTOCOL: &str = "2025-06-18";

/// The revisions it answers in, as a client asks.
const PROTOCOLS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// How many calls a key may make in a minute, every node together.
pub const CALLS_A_MINUTE: u32 = 30;

/// What the server tells a client of itself.
const INSTRUCTIONS: &str = "Reads what a merchant's shops hold on timada (catalog, margins, sales, ads, Search Console, Merchant Center, launch path, checkpoints, niche studies) and prepares changes as proposals. Nothing you prepare is applied until a person approves it on the dashboard. Start with list_shops: every other tool takes a shop's slug.";

/// Why a call was not answered.
#[derive(Debug)]
pub enum Refused {
    /// The key does not let the call through, or what it asks for is not
    /// there: told to the agent as the tool's own answer.
    Tool(String),
    /// The arguments do not fit the tool.
    Arguments(String),
    /// The store or a read model failed.
    Fault(anyhow::Error),
}

impl From<anyhow::Error> for Refused {
    fn from(err: anyhow::Error) -> Self {
        Self::Fault(err)
    }
}

/// Who calls, and what a tool reads with.
pub struct Caller<'a> {
    pub store: &'a Executor,
    pub db: &'a Db,
    pub user_id: &'a str,
    pub key: &'a AgentKeyState,
    pub now: u64,
}

impl Caller<'_> {
    /// The shops the key covers that are there, not suspended, and that
    /// the user is still a member of.
    pub async fn shops(&self) -> anyhow::Result<Vec<TenantState>> {
        let tenancy = timada_tenancy::Command(self.store.clone());
        let mut shops = Vec::with_capacity(self.key.shops.len());
        for shop in &self.key.shops {
            if let Some(tenant) = tenancy.load_tenant(&shop.tenant_id).await?
                && !tenant.is_suspended()
                && tenant.members.contains_key(self.user_id)
            {
                shops.push(tenant);
            }
        }
        Ok(shops)
    }

    /// The shop at `slug`, when the key covers `permissions` there and the
    /// user holds them now.
    pub async fn shop(&self, slug: &str, permissions: &[&str]) -> Result<TenantState, Refused> {
        let shop = self
            .shops()
            .await?
            .into_iter()
            .find(|shop| shop.slug == slug)
            .ok_or_else(|| {
                Refused::Tool(format!(
                    "The key covers no shop « {slug} ». list_shops names the ones it does."
                ))
            })?;
        for permission in permissions {
            if !self.key.allows(&shop.id, permission) {
                return Err(Refused::Tool(format!(
                    "The key does not cover {permission} in « {slug} »."
                )));
            }
            if !shop.can(self.user_id, permission) {
                return Err(Refused::Tool(format!(
                    "The key's user no longer holds {permission} in « {slug} »."
                )));
            }
        }
        Ok(shop)
    }
}

/// An answer to send back: the HTTP status, and the body when there is one.
#[derive(Debug, PartialEq)]
pub struct Answer {
    pub status: u16,
    pub body: Option<Value>,
}

impl Answer {
    fn result(id: &Value, result: Value) -> Self {
        Self {
            status: 200,
            body: Some(json!({ "jsonrpc": "2.0", "id": id, "result": result })),
        }
    }

    fn error(status: u16, id: &Value, code: i64, message: &str) -> Self {
        Self {
            status,
            body: Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": code, "message": message },
            })),
        }
    }
}

/// The token of an `Authorization: Bearer …` header.
pub fn bearer(authorization: Option<&str>) -> Option<&str> {
    let (scheme, token) = authorization?.trim().split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then_some(token.trim())
        .filter(|token| !token.is_empty())
}

/// Answers one request to the server: `body` as it came, under the key
/// `authorization` presents. Nothing is answered without a key that
/// stands.
pub async fn handle(
    store: &Executor,
    db: &Db,
    limits: &RateLimiter,
    authorization: Option<&str>,
    body: &[u8],
    now: u64,
) -> anyhow::Result<Answer> {
    let tenancy = timada_tenancy::Command(store.clone());
    let key = match bearer(authorization) {
        Some(token) => tenancy.agent_key_of(token).await?,
        None => None,
    };
    let Some((user_id, key)) = key else {
        return Ok(Answer::error(
            401,
            &Value::Null,
            -32001,
            "Present an agent key: Authorization: Bearer mk_…",
        ));
    };
    let Ok(request) = serde_json::from_slice::<Value>(body) else {
        return Ok(Answer::error(400, &Value::Null, -32700, "Not JSON."));
    };
    // One request a call: batches left the protocol.
    let Some(method) = request["method"].as_str() else {
        return Ok(Answer::error(
            400,
            &Value::Null,
            -32600,
            "Not a JSON-RPC request.",
        ));
    };
    let Some(id) = request.get("id").filter(|id| !id.is_null()) else {
        // A notification: taken, and nothing to say.
        return Ok(Answer {
            status: 202,
            body: None,
        });
    };
    let params = &request["params"];
    match method {
        "initialize" => {
            let asked = params["protocolVersion"].as_str().unwrap_or_default();
            let speaks = PROTOCOLS
                .into_iter()
                .find(|known| *known == asked)
                .unwrap_or(PROTOCOL);
            Ok(Answer::result(
                id,
                json!({
                    "protocolVersion": speaks,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "timada", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": INSTRUCTIONS,
                }),
            ))
        }
        "ping" => Ok(Answer::result(id, json!({}))),
        "tools/list" => Ok(Answer::result(
            id,
            json!({ "tools": TOOLS.iter().map(tools::Tool::listed).collect::<Vec<_>>() }),
        )),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            if !TOOLS.iter().any(|tool| tool.name == name) {
                return Ok(Answer::error(200, id, -32602, "No such tool."));
            }
            let window = Duration::from_secs(60);
            if !limits
                .check(&format!("mcp:{}", key.id), CALLS_A_MINUTE, window)
                .await
            {
                return Ok(Answer::error(
                    429,
                    id,
                    -32002,
                    "Too many calls with this key: wait a minute.",
                ));
            }
            let empty = json!({});
            let arguments = match &params["arguments"] {
                Value::Null => &empty,
                arguments => arguments,
            };
            let caller = Caller {
                store,
                db,
                user_id: &user_id,
                key: &key,
                now,
            };
            let outcome = tools::call(&caller, name, arguments).await;
            // Which shop it was about, for the log, when it named one the
            // key covers.
            let about = match arguments["shop"].as_str() {
                Some(slug) => caller
                    .shops()
                    .await?
                    .into_iter()
                    .find(|shop| shop.slug == slug)
                    .map(|shop| shop.id)
                    .unwrap_or_default(),
                None => String::new(),
            };
            let logged = tenancy
                .log_agent_call(&key.id, name, &about, outcome.is_ok(), now)
                .await;
            if let Err(err) = logged {
                // The call was answered; a log the store did not take is
                // said, not made the agent's problem.
                tracing::warn!(key = %key.id, %err, "an agent's call was not logged");
            }
            let text = |text: String, failed: bool| json!({ "content": [{ "type": "text", "text": text }], "isError": failed });
            match outcome {
                Ok(value) => Ok(Answer::result(id, text(value.to_string(), false))),
                Err(Refused::Tool(why)) => Ok(Answer::result(id, text(why, true))),
                Err(Refused::Arguments(why)) => Ok(Answer::error(200, id, -32602, &why)),
                Err(Refused::Fault(err)) => {
                    tracing::error!(key = %key.id, tool = %name, error = %err, "an agent's call failed");
                    Ok(Answer::error(200, id, -32603, "The server failed."))
                }
            }
        }
        _ => Ok(Answer::error(200, id, -32601, "No such method.")),
    }
}

#[cfg(test)]
mod tests;
