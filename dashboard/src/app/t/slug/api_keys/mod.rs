//! `/t/{slug}/api-keys`: the store API keys of a tenant, for members with
//! `api-keys:manage`. A key's token is shown once, right after its creation
//! (it travels in a private cookie across the redirect), then only its hint.
//! A key's scopes are chosen at creation and can be changed while it is
//! active. The storefront's own address, which the order confirmations
//! link to, is set here too.

pub mod create;
pub mod revoke;
pub mod scopes;
pub mod storefront;

use std::collections::HashMap;

use timada_tenancy::{
    API_KEY_SCOPES, MAX_API_KEY_LABEL_LEN, PERMISSION_API_KEYS_MANAGE, SCOPE_STORE_READ,
    api_keys_of_tenant,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant};
use crate::app::auth::Flash;
use crate::auth::cookies::{ApiKeyReveal, api_key_reveal, is_fresh};
use crate::auth::{now, user_labels};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::config::Config;
use crate::db::ReadPool;
use crate::media::Media;
use crate::ui::{
    Shell, Tone, describe_api_key_scope, document, empty_row, fact, facts, format_time, icons,
    notice, page_header, section_card, status_badge,
};

/// The form field of a scope checkbox: `scope:<scope>`.
const SCOPE_FIELD_PREFIX: &str = "scope:";

/// The scopes ticked in a posted key form.
pub fn checked_scopes(form: &HashMap<String, String>) -> Vec<String> {
    form.keys()
        .filter_map(|key| key.strip_prefix(SCOPE_FIELD_PREFIX))
        .map(str::to_owned)
        .collect()
}

#[topcoat::router::query_params(error = bad_request)]
struct ApiKeysQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn api_keys_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, Slug(slug.to_owned()))
        .query(Flash { error, message })
        .resolve(cx)
}

/// The token created by the request before this one, read once: the cookie
/// is removed whether or not it is still fresh.
fn take_reveal(cx: &Cx) -> Option<ApiKeyReveal> {
    let store = api_key_reveal(cx).parse().ok().flatten()?;
    let reveal = store.get();
    store.remove();
    is_fresh(reveal.issued_at, now()).then_some(reveal)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_API_KEYS_MANAGE)
        .await?;
    let query = query_params::<ApiKeysQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let reveal = take_reveal(cx);
    let store_api = app_context::<Config>(cx).store_api.clone();
    let keys = api_keys_of_tenant(app_context::<ReadPool>(cx), &access.tenant.id).await?;
    let media_url = app_context::<std::sync::Arc<Media>>(cx).tenant_url(&access.tenant.id);
    let api_url = app_context::<Config>(cx)
        .server
        .url
        .as_str()
        .trim_end_matches('/')
        .to_owned();
    let storefront_url = super::orders::orders(&access)?
        .storefront_address()
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .unwrap_or_default();
    let ids: Vec<String> = keys.iter().map(|k| k.created_by.clone()).collect();
    let labels = user_labels(cx, &ids).await?;
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("API keys · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "api_keys"),
            page_header(title: "API keys", subtitle: Some("What a storefront presents to connect to this tenant's store."))
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(reveal) = &reveal {
                <section class="mb-6" data-api-key-reveal=(&reveal.key_id)>
                    alert(
                        variant: AlertVariant::Warning,
                        icon(data: icons::API_KEYS)
                        <h2 class="col-start-2 font-medium tracking-tight">"Your new key " <q>(&reveal.label)</q></h2>
                        alert_description(
                            <p>"Copy it now: this is the only time it is shown. A storefront presents it when it connects to this tenant's store."</p>
                            <code class="mt-3 block select-all rounded-md border border-warning/40 bg-card px-3 py-2 font-mono text-sm text-foreground" data-api-key-token="">(&reveal.token)</code>
                            <p class="mt-3">
                                (match &store_api {
                                    Some(api) => match &api.public_addr {
                                        Some(addr) => format!("Storefronts connect to {addr} with it."),
                                        None => "Storefronts connect to this node's store API with it.".to_owned(),
                                    },
                                    None => "The store API is not enabled on this node yet; the key will work once it is.".to_owned(),
                                })
                            </p>
                        )
                    )
                </section>
            }

            section_card(
                title: Some("What a storefront needs"),
                description: Some("A storefront loads the product pictures from the picture address, and hands in shoppers' reviews at the dashboard address; set both alongside the key."),
                facts(
                    fact(
                        term: "Store address",
                        attrs: attributes! { data-store-addr="" },
                        (match &store_api {
                            Some(api) => api.public_addr.clone().unwrap_or_else(|| api.listen.to_string()),
                            None => "not enabled on this node".to_owned(),
                        })
                    )
                    fact(term: "Picture address", <code data-media-url="">(&media_url)</code>)
                    fact(term: "Dashboard address", <code data-api-url="">(&api_url)</code>)
                )
            )

            section_card(
                title: Some("Storefront address"),
                description: Some("The confirmation mailed to a shopper after payment links to the order at this address followed by /orders/ and the order's id. Leave it blank to mail confirmations without a link."),
                <form method="post" action=(href!(storefront::storefront, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="storefront-url" }, "Where shoppers find the shop")
                        input(attrs: attributes! {
                            id="storefront-url" name="url" type="url" maxlength="200"
                            value=(&storefront_url) placeholder="https://shop.example"
                        })
                    )
                    <div>
                        button(attrs: attributes! { type="submit" }, "Save address")
                    </div>
                </form>
            )

            section_card(
                title: Some("Keys"),
                description: Some("A key reads this tenant's event store; one with store:write also writes to it. Give a storefront that only shows the catalogue a read-only key, and one that sells store:read with store:checkout, which lets it fill carts and place orders and nothing else. Changing a key's scopes reconnects the stores using it. Revoke a key the moment it may have leaked."),
                table(
                    table_header(
                        table_row(
                            table_head("Label")
                            table_head("Key")
                            table_head("Scopes")
                            table_head("Created")
                            table_head("Status")
                            table_head(<span class="sr-only">"Actions"</span>)
                        )
                    )
                    table_body(
                        if keys.is_empty() {
                            empty_row(colspan: 6, text: "No key yet.")
                        }
                        for key in &keys {
                            table_row(
                                attrs: attributes! { data-api-key=(&key.hint) },
                                table_cell(attrs: attributes! { class="font-medium" }, (&key.label))
                                table_cell(<code>(&key.hint) "…"</code>)
                                table_cell(
                                    attrs: attributes! { class="whitespace-normal" data-api-key-scopes=(&key.scope_list) },
                                    if key.is_active() {
                                        <form method="post" action=(href!(scopes::scopes, Slug(slug.clone()))) class="flex flex-wrap items-center gap-x-3 gap-y-1">
                                            <input type="hidden" name="key_id" value=(&key.id)>
                                            for scope in API_KEY_SCOPES {
                                                <label class="flex items-center gap-1.5" title=(describe_api_key_scope(scope))>
                                                    checkbox(attrs: attributes! { name=(format!("{SCOPE_FIELD_PREFIX}{scope}")) value="on" checked=(key.has_scope(scope)) })
                                                    <code>(*scope)</code>
                                                </label>
                                            }
                                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Save scopes")
                                        </form>
                                    } else {
                                        <code class="text-muted-foreground">(&key.scope_list)</code>
                                    }
                                )
                                table_cell(
                                    <span class="block">(labels.get(&key.created_by).cloned().unwrap_or_else(|| key.created_by.clone()))</span>
                                    <time class="block text-xs text-muted-foreground" datetime=(key.created_at)>(format_time(key.created_at as u64))</time>
                                )
                                table_cell(
                                    attrs: attributes! { data-api-key-status=(if key.is_active() { "active" } else { "revoked" }) },
                                    if let Some(at) = key.revoked_at {
                                        status_badge(tone: Tone::Neutral, (format!("Revoked {}", format_time(at as u64))))
                                    } else {
                                        status_badge(tone: Tone::Success, "Active")
                                    }
                                )
                                table_cell(
                                    attrs: attributes! { class="text-right" },
                                    if key.is_active() {
                                        <form method="post" action=(href!(revoke::revoke, Slug(slug.clone())))>
                                            <input type="hidden" name="key_id" value=(&key.id)>
                                            button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Revoke")
                                        </form>
                                    }
                                )
                            )
                        }
                    )
                )
            )

            section_card(
                title: Some("Create a key"),
                <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="flex max-w-xl flex-col gap-4">
                    field(
                        field_label(attrs: attributes! { for="new-key-label" }, "Label")
                        input(attrs: attributes! {
                            id="new-key-label" name="label" type="text"
                            maxlength=(MAX_API_KEY_LABEL_LEN) required="" placeholder="Web storefront"
                        })
                    )
                    <fieldset class="flex flex-col gap-1.5 text-sm">
                        <legend class="mb-2 text-sm font-medium">"Scopes"</legend>
                        for scope in API_KEY_SCOPES {
                            <label class="flex items-center gap-2">
                                checkbox(attrs: attributes! { name=(format!("{SCOPE_FIELD_PREFIX}{scope}")) value="on" checked=(*scope == SCOPE_STORE_READ) })
                                <span>(describe_api_key_scope(scope)) " " <code class="text-muted-foreground">(*scope)</code></span>
                            </label>
                        }
                    </fieldset>
                    <div>
                        button(attrs: attributes! { type="submit" }, "Create key")
                    </div>
                </form>
            )
        )
    })
}
