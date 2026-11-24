//! `/t/{slug}/google`: the Google account the shop is connected to, for
//! members with `pages:read`; those with `pages:manage` connect it, choose
//! the shop's Search Console site and take the connection off. One
//! connection serves everything the shop reads from Google, so there is one
//! place to say yes and one "connect again". Read from the connection's own
//! stream, so a change shows at once; the sites to choose from are asked of
//! Google when the page lists them.

pub mod connect;
pub mod disconnect;
pub mod finish;
pub mod site;

use timada_google::{
    ACCESS_ADS, ACCESS_MERCHANT_CENTER, ACCESS_SEARCH_CONSOLE, Command, ConnectionState,
    GoogleError,
};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::cluster::Executor;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::checkbox::checkbox;
use crate::google::{Google, GoogleFault, OFFERED, Site};
use crate::ui::{
    LINK, Shell, Tone, document, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
};

#[derive(serde::Serialize)]
struct GoogleLink<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<&'a str>,
    /// Lists the sites to choose from, with one chosen already.
    #[serde(skip_serializing_if = "Option::is_none")]
    sites: Option<&'a str>,
}

#[topcoat::router::query_params(error = bad_request)]
struct GoogleQuery {
    error: Option<String>,
    message: Option<String>,
    sites: Option<String>,
}

/// The page, with a flash error code or message.
pub fn google_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// The gateway to the shops' connections, in the platform's streams.
pub fn google(cx: &Cx) -> Command<Executor> {
    Command(context::<Executor>(cx).clone())
}

/// The query-string code the pages show for a domain error.
pub fn error_code(err: &GoogleError) -> &'static str {
    err.code().unwrap_or("failed")
}

/// The query-string code the pages show when Google gave nothing.
pub fn fault_code(fault: &GoogleFault) -> &'static str {
    match fault {
        GoogleFault::Off => "google_disabled",
        GoogleFault::ConnectAgain => "google_connect_again",
        GoogleFault::Unavailable(_) => "google_unavailable",
        GoogleFault::Refused(_) => "google_refused",
    }
}

/// How a kind of access reads.
pub fn access_label(access: &str) -> &'static str {
    match access {
        ACCESS_SEARCH_CONSOLE => "Search Console (read only)",
        ACCESS_ADS => "Google Ads",
        ACCESS_MERCHANT_CENTER => "Merchant Center",
        _ => "Unknown",
    }
}

/// What a kind of access lets the shop do, for the member about to say yes.
fn access_hint(access: &str) -> &'static str {
    match access {
        ACCESS_SEARCH_CONSOLE => {
            "The queries your shop shows for on Google and the pages found, read once a day. Nothing is changed in Search Console."
        }
        _ => "",
    }
}

/// The Search Console sites the shop's account reads, asked of Google under
/// the connection's grant.
pub async fn sites_of(
    cx: &Cx,
    service: &Google,
    connection: &ConnectionState,
) -> std::result::Result<Vec<Site>, GoogleFault> {
    service
        .read(context::<Executor>(cx), connection, |access| async move {
            service.sites(&access).await
        })
        .await
}

/// Whether `site` is the one a shop at `address` (`https://shop.example`)
/// would have: that address, or its whole domain.
fn is_shop_site(site: &str, address: Option<&str>) -> bool {
    let Some(address) = address else {
        return false;
    };
    let address = address.trim_end_matches('/');
    let host = address
        .split_once("://")
        .map_or(address, |(_, rest)| rest)
        .trim_start_matches("www.");
    site.trim_end_matches('/') == address
        || site
            .strip_prefix("sc-domain:")
            .is_some_and(|domain| domain == host)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<GoogleQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let service = context::<Google>(cx);
    let enabled = service.enabled();
    let connection = google(cx)
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|connection| connection.connected);
    // The sites are asked of Google only to choose one: with none chosen
    // yet, or when the member asks to change it.
    let choosing = connection.as_ref().is_some_and(|connection| {
        can_manage
            && connection.reads(ACCESS_SEARCH_CONSOLE)
            && (connection.site.is_none() || query.sites.is_some())
    });
    let mut sites_fault = None;
    let mut sites = Vec::new();
    if let Some(connection) = connection.as_ref().filter(|_| choosing) {
        match sites_of(cx, service, connection).await {
            Ok(found) => sites = found,
            Err(fault) => sites_fault = Some(fault),
        }
    }
    // Met while listing the sites: the page says so at once.
    let lapsed = connection.as_ref().is_some_and(|c| c.lapsed)
        || sites_fault == Some(GoogleFault::ConnectAgain);
    let address = if choosing {
        timada_orders::Command::new(access.executor()?.clone(), access.tenant_id())
            .storefront_address()
            .await
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        None
    };
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Google account · {}", tenant.name);
    let state = match &connection {
        None => "none",
        Some(_) if lapsed => "lapsed",
        Some(_) => "connected",
    };
    let covered: Vec<&'static str> = connection
        .as_ref()
        .map(|connection| {
            connection
                .accesses
                .iter()
                .map(|access| access_label(access))
                .collect()
        })
        .unwrap_or_default();
    // What is left to turn on, and what a first connection asks for.
    let offered: Vec<(&'static str, &'static str, &'static str)> = OFFERED
        .iter()
        .filter(|kind| !connection.as_ref().is_some_and(|c| c.covers(kind)))
        .map(|kind| (*kind, access_label(kind), access_hint(kind)))
        .collect();
    let account = connection
        .as_ref()
        .map(|connection| connection.account.clone())
        .unwrap_or_default();
    let connected_at = connection.as_ref().map_or(0, |c| c.connected_at);
    let covers_search = connection
        .as_ref()
        .is_some_and(|c| c.covers(ACCESS_SEARCH_CONSOLE));
    let site = connection.as_ref().and_then(|c| c.site.clone());
    let choices: Vec<(String, bool, bool)> = sites
        .iter()
        .map(|found| {
            let chosen = match &site {
                Some(site) => *site == found.site,
                None => is_shop_site(&found.site, address.as_deref()),
            };
            (found.site.clone(), found.owner, chosen)
        })
        .collect();
    let sites_fault = sites_fault
        .filter(|fault| *fault != GoogleFault::ConnectAgain)
        .map(|fault| crate::ui::describe_error(fault_code(&fault)));
    let change_site = href!(index, slug: slug.clone())
        .query(GoogleLink {
            error: None,
            message: None,
            sites: Some("1"),
        })
        .resolve(cx);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "google"),
            page_header(
                title: "Google account",
                subtitle: Some("One connection to the Google account that owns your shop's site: what the shop reads from Google goes through it."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description("Connecting a Google account is not available on this installation: its operator has to set it up first.")
                )
            }
            if lapsed {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" class="mb-6" data-google-lapsed="" },
                    icon(data: icons::ERROR)
                    alert_description("Google no longer honours this connection: the account withdrew its access, or it went unused for too long. Nothing is read from Google until you connect again.")
                )
            }

            section_card(
                title: Some("Connection"),
                <div data-google=(state)>
                    if connection.is_some() {
                        <div class="mb-4">
                            facts(
                                if lapsed {
                                    fact(term: "Status", status_badge(tone: Tone::Danger, "Connect again"))
                                } else {
                                    fact(term: "Status", status_badge(tone: Tone::Success, "Connected"))
                                }
                                fact(term: "Account", attrs: attributes! { data-google-account="" }, (&account))
                                fact(term: "Access", attrs: attributes! { data-google-accesses="" }, (covered.join(", ")))
                                fact(term: "Since", <time datetime=(connected_at)>(format_time(connected_at))</time>)
                            )
                        </div>
                    } else {
                        <p class="mb-4 text-sm text-muted-foreground">"Not connected: nothing is read from Google for this shop."</p>
                    }
                    if can_manage && enabled {
                        <form method="post" action=(href!(connect::connect, slug: slug.clone())) class="flex max-w-2xl flex-col gap-4">
                            for (kind, label, hint) in &offered {
                                <div>
                                    <label class="flex items-center gap-2 text-sm font-medium">
                                        checkbox(attrs: attributes! { name=(*kind) value="on" checked="" aria-describedby=(format!("google-{kind}-hint")) })
                                        (*label)
                                    </label>
                                    <p id=(format!("google-{kind}-hint")) class="mt-1 text-sm text-muted-foreground">(*hint)</p>
                                </div>
                            }
                            <div>
                                if connection.is_none() {
                                    button(attrs: attributes! { type="submit" }, "Connect a Google account")
                                } else if lapsed || !offered.is_empty() {
                                    button(attrs: attributes! { type="submit" }, "Connect again")
                                } else {
                                    button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Connect again, or another account")
                                }
                            </div>
                            <p class="text-sm text-muted-foreground">"You are sent to Google to say yes, with the account that owns the shop's site, then brought back here. Connecting again keeps the access already given."</p>
                        </form>
                    }
                </div>
            )

            if covers_search && !lapsed {
                section_card(
                    title: Some("Search Console site"),
                    if let Some(site) = &site {
                        <div class="mb-4">
                            facts(
                                fact(term: "Site", attrs: attributes! { data-google-site="" }, <code>(site)</code>)
                            )
                        </div>
                    } else {
                        <p class="mb-4 text-sm text-muted-foreground" data-google-site-missing="">"No site chosen yet: say which of the account's sites is this shop's, and its queries and pages are read from the next day on."</p>
                    }
                    if let Some(fault) = sites_fault {
                        alert(
                            variant: AlertVariant::Warning,
                            attrs: attributes! { role="alert" class="mb-4" },
                            alert_description((fault))
                        )
                    }
                    if choosing && !choices.is_empty() {
                        <form method="post" action=(href!(site::site, slug: slug.clone())) class="flex max-w-2xl flex-col gap-3">
                            <fieldset class="flex flex-col gap-2">
                                <legend class="mb-2 text-sm font-medium">"The sites this account reads in Search Console"</legend>
                                for (name, owner, chosen) in &choices {
                                    <label class="flex items-center gap-2 text-sm">
                                        <input type="radio" name="site" value=(name) checked=(*chosen) required="" class="accent-primary">
                                        <code>(name)</code>
                                        if !*owner {
                                            <span class="text-muted-foreground">"(shared with the account)"</span>
                                        }
                                    </label>
                                }
                            </fieldset>
                            <div>button(attrs: attributes! { type="submit" }, "Save the site")</div>
                        </form>
                    } else if choosing && sites_fault.is_none() {
                        <p class="text-sm text-muted-foreground" data-google-no-site="">"This account reads no site in Search Console. Add the shop's address there and prove it is yours, or connect the account that owns it."</p>
                    } else if can_manage && site.is_some() {
                        <a href=(change_site) class=(LINK)>"Change the site"</a>
                    }
                )
            }

            if connection.is_some() && can_manage {
                section_card(
                    title: Some("Disconnect"),
                    <p class="mb-4 text-sm text-muted-foreground">
                        "Disconnecting forgets the access kept here: nothing more is read from Google for this shop. To withdraw the access at Google too, open "
                        <a href="https://myaccount.google.com/connections" class=(LINK) rel="noreferrer">"the account's connections"</a>
                        ": that ends it for every shop connected to the same account."
                    </p>
                    <form method="post" action=(href!(disconnect::disconnect, slug: slug.clone()))>
                        button(variant: ButtonVariant::Destructive, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Disconnect")
                    </form>
                )
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_site_of_the_shops_address_is_the_one_proposed() {
        let address = Some("https://www.shop.example");
        assert!(is_shop_site("https://www.shop.example/", address));
        assert!(is_shop_site("sc-domain:shop.example", address));
        assert!(!is_shop_site("https://other.example/", address));
        assert!(!is_shop_site("sc-domain:other.example", address));
        assert!(!is_shop_site("https://www.shop.example/", None));
    }
}
