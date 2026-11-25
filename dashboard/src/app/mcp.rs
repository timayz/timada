//! `POST /mcp`: the dashboard's MCP server, for an assistant that presents
//! an agent key ([`crate::mcp`] does the work). Off unless the operator
//! turns it on (`[mcp] enabled`): 404 then. Any node may take a call.
//!
//! Answers 200 with the JSON-RPC response; 202 for a notification; 400 for
//! what is no request; 401 without a key that stands; 429 past the key's
//! calls a minute.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use topcoat::{
    Result,
    context::{Cx, context},
    router::{StatusCode, content::Json, error::not_found, request::headers, route},
};

use crate::cluster::Executor;
use crate::config::Config;
use crate::db::Db;
use crate::rate_limit::RateLimiter;

#[route(POST)]
pub async fn mcp(cx: &Cx, body: Bytes) -> Result<(StatusCode, Json<Value>)> {
    if !context::<Config>(cx).mcp.enabled {
        return Err(not_found().into());
    }
    let authorization = headers(cx)
        .get("authorization")
        .and_then(|value| value.to_str().ok());
    let answer = crate::mcp::handle(
        context::<Executor>(cx),
        context::<Db>(cx),
        context::<Arc<RateLimiter>>(cx),
        authorization,
        &body,
        crate::auth::now(),
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    Ok((
        StatusCode::from_u16(answer.status).unwrap_or(StatusCode::OK),
        Json(answer.body.unwrap_or(Value::Null)),
    ))
}
