//! `/auth/agent-keys`: the keys the signed-in user gave to agents, for the
//! dashboard's MCP server. A key is made for the shops and the permissions
//! the user ticks, never more than they hold; its token is shown once. The
//! page lists each key with what it covers, when it was last used and what
//! it last asked for, and withdraws one. 404 while the server is off.

pub mod mint;
pub mod withdraw;

use timada_tenancy::{PERMISSIONS, tenants_of_user};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{error::not_found, href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::auth::Flash;
use crate::app::tenants::tenancy;
use crate::auth::cookies::{ApiKeyReveal, agent_key_reveal, is_fresh};
use crate::auth::{now, require_user};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::label::label;
use crate::config::Config;
use crate::db::Db;
use crate::ui::{
    Shell, describe_permission, document, format_time, icons, notice, page_header, section_card,
};

/// How many of a key's last calls the page lists.
const CALLS_LISTED: usize = 20;

#[topcoat::router::query_params(error = bad_request)]
struct KeysQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn agent_keys_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(index).query(Flash { error, message }).resolve(cx)
}

/// What the checkbox of a permission in a shop is named: the shop's id and
/// the permission follow.
pub const GRANT_PREFIX: &str = "grant:";

/// The token just made, read once: the cookie is removed whether or not
/// it is still fresh.
fn take_reveal(cx: &Cx) -> Option<ApiKeyReveal> {
    let store = agent_key_reveal(cx).parse().ok().flatten()?;
    let reveal = store.get();
    store.remove();
    is_fresh(reveal.issued_at, now()).then_some(reveal)
}

/// A shop the user may cover with a key, with what they hold there.
struct Shop {
    id: String,
    name: String,
    permissions: Vec<&'static str>,
}

/// A call of a key's log.
struct Call {
    tool: String,
    shop: String,
    ok: bool,
    at: u64,
}

/// A key as the page shows it.
struct Key {
    id: String,
    label: String,
    hint: String,
    /// Each shop it covers, by name, with its permissions there.
    covers: Vec<(String, String)>,
    minted_at: u64,
    withdrawn_at: Option<u64>,
    /// The last day it was used on, and how many calls it made then.
    last_day: Option<(String, u32)>,
    calls: Vec<Call>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    if !context::<Config>(cx).mcp.enabled {
        return Err(not_found().into());
    }
    let query = query_params::<KeysQuery>(cx)?;
    let reveal = take_reveal(cx);
    let failed = topcoat::Error::from_anyhow;
    let tenancy = tenancy(cx);
    let mut shops = Vec::new();
    let mut names = std::collections::HashMap::new();
    for membership in tenants_of_user(context::<Db>(cx), &user.id)
        .await
        .map_err(failed)?
    {
        names.insert(membership.tenant_id.clone(), membership.name.clone());
        if membership.suspended {
            continue;
        }
        // What the user holds is read from the shop itself, not the list.
        let Some(tenant) = tenancy
            .load_tenant(&membership.tenant_id)
            .await
            .map_err(failed)?
        else {
            continue;
        };
        let held: Vec<&'static str> = PERMISSIONS
            .iter()
            .copied()
            .filter(|permission| tenant.can(&user.id, permission))
            .collect();
        if !held.is_empty() {
            shops.push(Shop {
                id: tenant.id,
                name: tenant.name,
                permissions: held,
            });
        }
    }
    let held = tenancy.load_agent_keys(&user.id).await.map_err(failed)?;
    let mut keys = Vec::new();
    for key in held.iter().flat_map(|held| held.keys.iter().rev()) {
        let day = tenancy.agent_key_last_day(&key.id).await.map_err(failed)?;
        let log = match &day {
            Some(day) => tenancy
                .load_agent_calls(&key.id, day)
                .await
                .map_err(failed)?,
            None => None,
        };
        keys.push(Key {
            id: key.id.clone(),
            label: key.label.clone(),
            hint: key.hint.clone(),
            covers: key
                .shops
                .iter()
                .map(|shop| {
                    (
                        names
                            .get(&shop.tenant_id)
                            .cloned()
                            .unwrap_or_else(|| shop.tenant_id.clone()),
                        shop.permissions.join(", "),
                    )
                })
                .collect(),
            minted_at: key.minted_at,
            withdrawn_at: key.withdrawn_at,
            last_day: day.zip(log.as_ref().map(|log| log.count)),
            calls: log
                .iter()
                .flat_map(|log| log.last.iter().rev().take(CALLS_LISTED))
                .map(|call| Call {
                    tool: call.tool.clone(),
                    shop: names.get(&call.tenant_id).cloned().unwrap_or_default(),
                    ok: call.ok,
                    at: call.at,
                })
                .collect(),
        });
    }
    let address = href!(crate::app::mcp::mcp).resolve(cx);
    let mint_url = href!(mint::mint).resolve(cx);
    let withdraw_url = href!(withdraw::withdraw).resolve(cx);

    Ok(view! {
        document(
            title: "Agent keys",
            shell: Shell::page("account"),
            page_header(
                title: "Agent keys",
                subtitle: Some("A key lets an assistant read your shops and prepare changes through timada's MCP server, as you, within the shops and permissions you give it. What it prepares waits for a person: an agent never approves, publishes, refunds or touches payments."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(reveal) = &reveal {
                <section class="mb-6" data-agent-key-reveal=(&reveal.key_id)>
                    alert(
                        variant: AlertVariant::Warning,
                        icon(data: icons::API_KEYS)
                        <h2 class="col-start-2 font-medium tracking-tight">"Your new key " <q>(&reveal.label)</q></h2>
                        alert_description(
                            <p>"Copy it now: this is the only time it is shown. The assistant presents it as " <code>"Authorization: Bearer …"</code> "."</p>
                            <code class="mt-3 block select-all break-all rounded-md border border-warning/40 bg-card px-3 py-2 font-mono text-sm text-foreground" data-agent-key-token="">(&reveal.token)</code>
                        )
                    )
                </section>
            }

            section_card(
                title: Some("Where an assistant connects"),
                <p class="text-sm">"The MCP server answers at " <code data-mcp-address="">(&address)</code> " on this dashboard's own address, over HTTP."</p>
            )

            section_card(
                title: Some("Your keys"),
                if keys.is_empty() {
                    <p class="text-sm text-muted-foreground">"No key yet."</p>
                }
                <ul class="flex flex-col gap-6">
                    for key in &keys {
                        <li data-agent-key=(&key.id) data-agent-key-state=(if key.withdrawn_at.is_some() { "withdrawn" } else { "active" })>
                            <h3 class="font-medium">
                                (&key.label) " " <code class="text-sm text-muted-foreground">(&key.hint)</code>
                            </h3>
                            <ul class="mt-1 text-sm text-muted-foreground" data-agent-key-covers="">
                                for (shop, permissions) in &key.covers {
                                    <li>(shop) ": " (permissions)</li>
                                }
                            </ul>
                            <p class="mt-1 text-sm text-muted-foreground">
                                "Made " <time datetime=(key.minted_at)>(format_time(key.minted_at))</time>
                                if let Some(at) = key.withdrawn_at {
                                    " · withdrawn " <time datetime=(at)>(format_time(at))</time>
                                }
                                if let Some((day, count)) = &key.last_day {
                                    " · last used on " <span data-agent-key-last-day="">(day)</span> ", " <span data-agent-key-calls=(count)>(count) " calls"</span>
                                } else {
                                    " · never used"
                                }
                            </p>
                            if !key.calls.is_empty() {
                                <details class="mt-2 text-sm">
                                    <summary class="cursor-pointer">"Its last calls"</summary>
                                    <ul class="mt-2 flex flex-col gap-1 text-muted-foreground" data-agent-key-log="">
                                        for call in &key.calls {
                                            <li>
                                                <time datetime=(call.at)>(format_time(call.at))</time> " · " (&call.tool)
                                                if !call.shop.is_empty() { " · " (&call.shop) }
                                                if !call.ok { " · refused" }
                                            </li>
                                        }
                                    </ul>
                                </details>
                            }
                            if key.withdrawn_at.is_none() {
                                <form method="post" action=(&withdraw_url) class="mt-3">
                                    <input type="hidden" name="key_id" value=(&key.id) />
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" class="text-destructive-text" }, "Withdraw")
                                </form>
                            }
                        </li>
                    }
                </ul>
            )

            section_card(
                title: Some("Make a key"),
                description: Some("Tick what the assistant may do in each shop. A key never covers more than you hold there, and loses what you lose."),
                if shops.is_empty() {
                    <p class="text-sm text-muted-foreground">"You are a member of no shop yet."</p>
                } else {
                    <form method="post" action=(&mint_url) class="flex max-w-3xl flex-col gap-5" data-agent-key-form="">
                        field(
                            field_label(attrs: attributes! { for="agent-key-label" }, "Label")
                            input(attrs: attributes! { id="agent-key-label" name="label" type="text" required="" maxlength="40" placeholder="Claude on my laptop" })
                        )
                        for shop in &shops {
                            <fieldset class="flex flex-col gap-2" data-agent-key-shop=(&shop.id)>
                                <legend class="mb-1 font-medium">(&shop.name)</legend>
                                for permission in &shop.permissions {
                                    label(
                                        checkbox(attrs: attributes! { name=(format!("{GRANT_PREFIX}{}:{permission}", shop.id)) value="on" })
                                        <span><code>(permission)</code> " " <span class="text-muted-foreground">(describe_permission(permission))</span></span>
                                    )
                                }
                            </fieldset>
                        }
                        <div>button(attrs: attributes! { type="submit" }, "Make the key")</div>
                    </form>
                }
            )
        )
    })
}
