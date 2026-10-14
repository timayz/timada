//! `/t/{slug}/api-keys`: the store API keys of a tenant, for members with
//! `api-keys:manage`. A key's token is shown once, right after its creation
//! (it travels in a private cookie across the redirect), then only its hint.
//! A key's scopes are chosen at creation and can be changed while it is
//! active.

pub mod create;
pub mod revoke;
pub mod scopes;

use std::collections::HashMap;

use timada_tenancy::{
    API_KEY_SCOPES, MAX_API_KEY_LABEL_LEN, PERMISSION_API_KEYS_MANAGE, SCOPE_STORE_READ,
    api_keys_of_tenant,
};
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use super::{Slug, TenantAccess, TenantNav, require_tenant, tenant_nav};
use crate::app::auth::Flash;
use crate::auth::cookies::{ApiKeyReveal, api_key_reveal, is_fresh};
use crate::auth::{now, user_labels};
use crate::config::Config;
use crate::db::ReadPool;
use crate::media::Media;
use crate::ui::{describe_api_key_scope, document, format_time, notice};

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
    let TenantAccess { tenant, .. } = access;
    let reveal = take_reveal(cx);
    let store_api = app_context::<Config>(cx).store_api.clone();
    let keys = api_keys_of_tenant(app_context::<ReadPool>(cx), &tenant.id).await?;
    let media_url = app_context::<std::sync::Arc<Media>>(cx).tenant_url(&tenant.id);
    let ids: Vec<String> = keys.iter().map(|k| k.created_by.clone()).collect();
    let labels = user_labels(cx, &ids).await?;
    let slug = tenant.slug.clone();
    let title = format!("API keys · {}", tenant.name);

    Ok(view! {
        document(
            title: &title,
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"API keys"</h1>
                tenant_nav(tenant: &tenant, nav: &nav, current: "api_keys")
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some(reveal) = &reveal {
                <section class="mb-8 rounded border border-amber-300 bg-amber-50 p-4" data-api-key-reveal=(&reveal.key_id)>
                    <h2 class="text-lg font-medium">"Your new key " <q>(&reveal.label)</q></h2>
                    <p class="mt-1 text-sm text-amber-900">
                        "Copy it now: this is the only time it is shown. A storefront presents it when it connects to this tenant's store."
                    </p>
                    <code class="mt-3 block select-all rounded border border-amber-200 bg-white px-3 py-2 font-mono text-sm" data-api-key-token="">(&reveal.token)</code>
                    <p class="mt-3 text-sm text-amber-900">
                        (match &store_api {
                            Some(api) => match &api.public_addr {
                                Some(addr) => format!("Storefronts connect to {addr} with it."),
                                None => "Storefronts connect to this node's store API with it.".to_owned(),
                            },
                            None => "The store API is not enabled on this node yet; the key will work once it is.".to_owned(),
                        })
                    </p>
                </section>
            }

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"What a storefront needs"</h2>
                <dl class="grid grid-cols-[10rem_1fr] gap-y-1 text-sm">
                    <dt class="text-neutral-500">"Store address"</dt>
                    <dd data-store-addr="">
                        (match &store_api {
                            Some(api) => api.public_addr.clone().unwrap_or_else(|| api.listen.to_string()),
                            None => "not enabled on this node".to_owned(),
                        })
                    </dd>
                    <dt class="text-neutral-500">"Picture address"</dt>
                    <dd><code data-media-url="">(&media_url)</code></dd>
                </dl>
                <p class="mt-3 text-sm text-neutral-500">
                    "A storefront loads the product pictures from the picture address; set it alongside the key."
                </p>
            </section>

            <section class="mb-8 rounded border border-neutral-200 bg-white p-4">
                <p class="mb-3 text-sm text-neutral-500">
                    "A key reads this tenant's event store; one with store:write also writes to it. Give a storefront that only shows the catalogue a read-only key, and one that sells store:read with store:checkout, which lets it fill carts and place orders and nothing else. Changing a key's scopes reconnects the stores using it. Revoke a key the moment it may have leaked."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Label"</th>
                            <th class="py-2 font-medium">"Key"</th>
                            <th class="py-2 font-medium">"Scopes"</th>
                            <th class="py-2 font-medium">"Created"</th>
                            <th class="py-2 font-medium">"Status"</th>
                            <th class="py-2 font-medium"><span class="sr-only">"Actions"</span></th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        if keys.is_empty() {
                            <tr><td class="py-2 text-neutral-500" colspan="6">"No key yet."</td></tr>
                        }
                        for key in &keys {
                            <tr data-api-key=(&key.hint)>
                                <td class="py-2">(&key.label)</td>
                                <td class="py-2"><code>(&key.hint) "…"</code></td>
                                <td class="py-2" data-api-key-scopes=(&key.scope_list)>
                                    if key.is_active() {
                                        <form method="post" action=(href!(scopes::scopes, Slug(slug.clone()))) class="flex flex-wrap items-center gap-x-3 gap-y-1">
                                            <input type="hidden" name="key_id" value=(&key.id)>
                                            for scope in API_KEY_SCOPES {
                                                <label class="flex items-center gap-1" title=(describe_api_key_scope(scope))>
                                                    <input type="checkbox" name=(format!("{SCOPE_FIELD_PREFIX}{scope}")) value="on" checked=(key.has_scope(scope))>
                                                    <code>(*scope)</code>
                                                </label>
                                            }
                                            <button class="rounded border border-neutral-300 px-2 py-1 text-xs hover:bg-neutral-100">"Save scopes"</button>
                                        </form>
                                    } else {
                                        <code class="text-neutral-500">(&key.scope_list)</code>
                                    }
                                </td>
                                <td class="py-2">
                                    (labels.get(&key.created_by).cloned().unwrap_or_else(|| key.created_by.clone()))
                                    ", " <time datetime=(key.created_at)>(format_time(key.created_at as u64))</time>
                                </td>
                                <td class="py-2" data-api-key-status=(if key.is_active() { "active" } else { "revoked" })>
                                    (match key.revoked_at {
                                        None => "Active".to_owned(),
                                        Some(at) => format!("Revoked {}", format_time(at as u64)),
                                    })
                                </td>
                                <td class="py-2 text-right">
                                    if key.is_active() {
                                        <form method="post" action=(href!(revoke::revoke, Slug(slug.clone())))>
                                            <input type="hidden" name="key_id" value=(&key.id)>
                                            <button class="rounded border border-red-700 px-2 py-1 text-xs text-red-700 hover:bg-red-50">"Revoke"</button>
                                        </form>
                                    }
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>

            <section class="rounded border border-neutral-200 bg-white p-4">
                <h2 class="mb-2 text-lg font-medium">"Create a key"</h2>
                <form method="post" action=(href!(create::create, Slug(slug.clone()))) class="space-y-3">
                    <label class="block text-sm" for="new-key-label">"Label"</label>
                    <input id="new-key-label" name="label" class="mt-1 w-full rounded border border-neutral-300 px-3 py-2"
                        type="text" maxlength=(MAX_API_KEY_LABEL_LEN) required="" placeholder="Web storefront">
                    <fieldset class="space-y-1 text-sm">
                        <legend class="mb-1 text-sm">"Scopes"</legend>
                        for scope in API_KEY_SCOPES {
                            <label class="flex items-center gap-2">
                                <input type="checkbox" name=(format!("{SCOPE_FIELD_PREFIX}{scope}")) value="on" checked=(*scope == SCOPE_STORE_READ)>
                                <span>(describe_api_key_scope(scope)) " " <code class="text-neutral-500">(*scope)</code></span>
                            </label>
                        }
                    </fieldset>
                    <button class="rounded bg-neutral-900 px-4 py-2 font-medium text-white hover:bg-neutral-700">"Create key"</button>
                </form>
            </section>
        )
    })
}
