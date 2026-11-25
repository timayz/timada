//! `/t/{slug}/merchant-center`: what Merchant Center says of the shop's
//! products, for members with `pages:read`: how many Google accepts, still
//! looks at or refuses, for Shopping ads and for the free listings, what it
//! holds against the account, and the products it does not simply accept,
//! each with Google's reason and the field of the feed it names. Read once
//! a day through the shop's Google connection (`crate::merchant_passes`),
//! and when a member with `pages:manage` asks. Read from the read model;
//! where the daily read stands is the watch's own stream.

pub mod read;

use std::collections::HashMap;

use timada_catalog::feed_items;
use timada_google::{
    ACCESS_MERCHANT_CENTER, MERCHANT_ACCEPTED, MERCHANT_PENDING, MERCHANT_REFUSED,
    MerchantIssueRow, MerchantTally, merchant_account_issues, merchant_issues, merchant_problems,
    merchant_standing,
};
use timada_tenancy::{PERMISSION_PAGES_MANAGE, PERMISSION_PAGES_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::google::google;
use super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, document, empty_row, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
};

/// How many products Google does not simply accept the page lists.
const LISTED: i64 = 200;

#[topcoat::router::query_params(error = bad_request)]
struct MerchantQuery {
    error: Option<String>,
    message: Option<String>,
}

/// How a standing reads, and the colour it wears.
pub fn standing_label(status: &str) -> (&'static str, Tone) {
    match status {
        MERCHANT_ACCEPTED => ("Accepted", Tone::Success),
        MERCHANT_PENDING => ("Pending", Tone::Warning),
        MERCHANT_REFUSED => ("Refused", Tone::Danger),
        _ => ("Not sent", Tone::Neutral),
    }
}

/// Where Google has a product, for its own page: the standing of the worst
/// off of its items of the feed (the product itself, or each of its
/// variants), `(Shopping ads, free listings)`. `None` when the shop reads
/// no Merchant Center account, or the last read does not name any of them.
pub async fn product_standing(
    db: &Db,
    tenant_id: &str,
    item_ids: &[String],
) -> anyhow::Result<Option<(String, String)>> {
    let Some(account_id) = timada_google::merchant_account_of(db, tenant_id).await? else {
        return Ok(None);
    };
    let mut worst = None;
    for item_id in item_ids {
        let found = timada_google::merchant_product(db, tenant_id, &account_id, item_id).await?;
        if let Some(found) = found
            && worst
                .as_ref()
                .is_none_or(|worst: &timada_google::MerchantProduct| found.rank < worst.rank)
        {
            worst = Some(found);
        }
    }
    Ok(worst.map(|row| (row.ads, row.listings)))
}

/// How what Google holds against the account reads.
fn account_severity(severity: &str) -> (&'static str, AlertVariant) {
    match severity {
        "critical" => ("Nothing is shown", AlertVariant::Destructive),
        "error" => ("To fix", AlertVariant::Warning),
        _ => ("Suggestion", AlertVariant::Neutral),
    }
}

/// One product Google does not simply accept.
struct Problem {
    item_id: String,
    name: String,
    href: Option<String>,
    ads: String,
    listings: String,
    issues: Vec<MerchantIssueRow>,
}

/// A place's counts, as its facts show them.
struct Counts {
    key: &'static str,
    title: &'static str,
    tally: MerchantTally,
}

/// Where the shop stands with Merchant Center.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Standing {
    /// No connection, or one that does not cover Merchant Center.
    NotConnected,
    /// Google no longer honours the connection.
    Lapsed,
    /// Connected, with no account chosen.
    NoAccount,
    /// Read, or about to be.
    Read,
}

impl Standing {
    fn key(self) -> &'static str {
        match self {
            Self::NotConnected => "not-connected",
            Self::Lapsed => "lapsed",
            Self::NoAccount => "no-account",
            Self::Read => "read",
        }
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access.require_permission(cx, PERMISSION_PAGES_READ).await?;
    let query = query_params::<MerchantQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_PAGES_MANAGE).await?;
    let command = google(cx);
    let connection = command
        .load_connection(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|connection| connection.covers(ACCESS_MERCHANT_CENTER));
    let account = connection.as_ref().and_then(|c| c.merchant.clone());
    let standing = match &connection {
        None => Standing::NotConnected,
        Some(connection) if connection.lapsed => Standing::Lapsed,
        Some(_) if account.is_none() => Standing::NoAccount,
        Some(_) => Standing::Read,
    };
    // Where the daily read stands, for the account it is about.
    let watch = command
        .load_merchant_watch(access.tenant_id())
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .filter(|watch| Some(&watch.account_id) == account.as_ref().map(|a| &a.account_id));
    let db = context::<Db>(cx);
    let tenant_id = access.tenant_id().to_owned();
    let slug = access.tenant.slug.clone();
    let failed = topcoat::Error::from_anyhow;
    let mut held = None;
    let mut account_issues = Vec::new();
    let mut problems = Vec::new();
    if let Some(account) = &account {
        let account_id = account.account_id.as_str();
        // What a lapsed connection read before stays readable.
        held = merchant_standing(db, &tenant_id, account_id)
            .await
            .map_err(failed)?;
        account_issues = merchant_account_issues(db, &tenant_id, account_id)
            .await
            .map_err(failed)?;
        let rows = merchant_problems(db, &tenant_id, account_id, LISTED)
            .await
            .map_err(failed)?;
        if !rows.is_empty() {
            let mut issues: HashMap<String, Vec<MerchantIssueRow>> = HashMap::new();
            for issue in merchant_issues(db, &tenant_id, account_id)
                .await
                .map_err(failed)?
            {
                issues.entry(issue.item_id.clone()).or_default().push(issue);
            }
            let items = feed_items(db, &tenant_id).await.map_err(failed)?;
            for row in rows {
                let product = items.product_of(&row.item_id);
                problems.push(Problem {
                    name: product.map_or_else(|| row.item_id.clone(), |(_, name)| name.to_owned()),
                    href: product.map(|(product_id, _)| {
                        href!(
                            super::products::product_id::index,
                            slug: slug.clone(),
                            super::products::product_id::product_id: product_id.to_owned()
                        )
                        .resolve(cx)
                    }),
                    issues: issues.remove(&row.item_id).unwrap_or_default(),
                    item_id: row.item_id,
                    ads: row.ads,
                    listings: row.listings,
                });
            }
        }
    }
    let TenantAccess { tenant, .. } = access;
    let title = format!("Merchant Center · {}", tenant.name);
    let google_page = href!(super::google::index, slug: slug.clone()).resolve(cx);
    let failure = watch
        .as_ref()
        .filter(|_| standing == Standing::Read)
        .and_then(|watch| Some((watch.last_error.clone()?, watch.next_at)));
    let checked_at = watch.as_ref().map_or(0, |watch| watch.checked_at);
    let account_label = account
        .as_ref()
        .map(|account| {
            if account.name.is_empty() {
                account.account_id.clone()
            } else {
                format!("{} ({})", account.name, account.account_id)
            }
        })
        .unwrap_or_default();
    let counts: Vec<Counts> = held
        .iter()
        .flat_map(|held| {
            [
                Counts {
                    key: "ads",
                    title: "Shopping ads",
                    tally: held.ads,
                },
                Counts {
                    key: "listings",
                    title: "Free listings",
                    tally: held.listings,
                },
            ]
        })
        .collect();
    let products = held.map_or(0, |held| held.products());
    let all_accepted = held.is_some_and(|held| held.all_accepted_for_ads());
    let issues: Vec<(&'static str, AlertVariant, String, String, String)> = account_issues
        .into_iter()
        .map(|issue| {
            let (label, variant) = account_severity(&issue.severity);
            (
                label,
                variant,
                issue.title,
                issue.detail,
                issue.documentation,
            )
        })
        .collect();
    let problems_note = format!(
        "The products Google does not simply accept, the refused ones first, {LISTED} at most. A product refused in one country reads as refused. The field is the feed's own name for what Google points at."
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "merchant_center"),
            page_header(
                title: "Merchant Center",
                subtitle: Some("Whether Google accepts your products, for Shopping ads and for the free listings, and why not. Read once a day through the shop's Google account; nothing is changed in Merchant Center."),
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            <div data-merchant=(standing.key())>
                if standing == Standing::NotConnected {
                    section_card(
                        title: Some("Not connected"),
                        <p class="text-sm text-muted-foreground">
                            "Connect the Google account that manages your shop's Merchant Center account, on the "
                            <a href=(&google_page) class=(LINK)>"Google account"</a>
                            " page, and tick Merchant Center: where its products stand is read here from the next pass on."
                        </p>
                    )
                }
                if standing == Standing::Lapsed {
                    alert(
                        variant: AlertVariant::Destructive,
                        attrs: attributes! { role="alert" class="mb-6" data-merchant-lapsed="" },
                        icon(data: icons::ERROR)
                        alert_description(
                            "Google no longer honours the shop's connection: nothing new is read until you "
                            <a href=(&google_page) class=(LINK)>"connect again"</a>
                            ". What was read before stays below."
                        )
                    )
                }
                if standing == Standing::NoAccount {
                    section_card(
                        title: Some("No account chosen"),
                        <p class="text-sm text-muted-foreground">
                            "The shop is connected to Google. Say which Merchant Center account takes this shop's feed on the "
                            <a href=(&google_page) class=(LINK)>"Google account"</a>
                            " page."
                        </p>
                    )
                }
                if let Some((reason, next_at)) = &failure {
                    alert(
                        variant: AlertVariant::Warning,
                        attrs: attributes! { role="alert" class="mb-6" data-merchant-failure="" },
                        icon(data: icons::WARNING)
                        alert_description(
                            "The last read of Merchant Center failed: " (reason) ". It is tried again from "
                            <time datetime=(*next_at)>(format_time(*next_at))</time> "."
                        )
                    )
                }
            </div>

            if account.is_some() {
                section_card(
                    title: Some("Last read"),
                    if checked_at > 0 {
                        <p class="text-sm text-muted-foreground">
                            (&account_label) ", read on "
                            <time datetime=(checked_at) data-merchant-checked="">(format_time(checked_at))</time>
                            ": " <span data-merchant-products=(products)>(products)</span> " products."
                        </p>
                    } else if standing == Standing::Read && failure.is_none() {
                        <p class="text-sm text-muted-foreground" data-merchant-waiting="">"Nothing read yet for " (&account_label) ": the first read is on its way."</p>
                    } else {
                        <p class="text-sm text-muted-foreground">"Nothing read yet for " (&account_label) "."</p>
                    }
                    if can_manage && standing == Standing::Read {
                        <form method="post" action=(href!(read::read, slug: slug.clone())) class="mt-4">
                            button(variant: ButtonVariant::Outline, size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Read now")
                        </form>
                    }
                )
            }

            for (label, variant, title, detail, documentation) in &issues {
                alert(
                    variant: *variant,
                    attrs: attributes! { role="alert" class="mb-6" data-merchant-account-issue=(title) },
                    icon(data: icons::WARNING)
                    alert_description(
                        <strong>(*label) ": " (title)</strong>
                        if !detail.is_empty() { " " (detail) }
                        if !documentation.is_empty() {
                            " " <a href=(documentation) class=(LINK) rel="noreferrer">"What Google says"</a>
                        }
                    )
                )
            }

            if held.is_some() {
                section_card(
                    title: Some("Where the products stand"),
                    description: Some("Ads wait for every product to be accepted for Shopping ads."),
                    attrs: attributes! { data-merchant-all-accepted=(if all_accepted { "yes" } else { "no" }) },
                    <div class="grid gap-6 sm:grid-cols-2">
                        for place in &counts {
                            <div data-merchant-counts=(place.key)>
                                <h3 class="mb-2 text-sm font-medium">(place.title)</h3>
                                facts(
                                    fact(term: "Accepted", attrs: attributes! { data-accepted=(place.tally.accepted) }, (place.tally.accepted))
                                    fact(term: "Pending", attrs: attributes! { data-pending=(place.tally.pending) }, (place.tally.pending))
                                    fact(term: "Refused", attrs: attributes! { data-refused=(place.tally.refused) }, (place.tally.refused))
                                    fact(term: "Not sent", attrs: attributes! { data-absent=(place.tally.absent) }, (place.tally.absent))
                                )
                            </div>
                        }
                    </div>
                )

                section_card(
                    title: Some("To look at"),
                    description: Some(&problems_note),
                    table(
                        attrs: attributes! { data-merchant-problems="" },
                        table_header(
                            table_row(
                                table_head("Product")
                                table_head("Shopping ads")
                                table_head("Free listings")
                                table_head("What Google says")
                            )
                        )
                        table_body(
                            if problems.is_empty() {
                                empty_row(colspan: 4, text: "Google holds nothing against any product.")
                            }
                            for row in &problems {
                                table_row(
                                    attrs: attributes! { data-merchant-problem=(&row.item_id) },
                                    table_cell(
                                        if let Some(href) = &row.href {
                                            <a class=(LINK) href=(href)>(&row.name)</a>
                                        } else {
                                            (&row.name)
                                        }
                                    )
                                    table_cell(attrs: attributes! { data-ads=(&row.ads) }, status_badge(tone: standing_label(&row.ads).1, (standing_label(&row.ads).0)))
                                    table_cell(attrs: attributes! { data-listings=(&row.listings) }, status_badge(tone: standing_label(&row.listings).1, (standing_label(&row.listings).0)))
                                    table_cell(
                                        if row.issues.is_empty() {
                                            <span class="text-muted-foreground">"No reason given yet."</span>
                                        }
                                        <ul class="flex flex-col gap-2">
                                            for issue in &row.issues {
                                                <li data-merchant-issue=(&issue.code)>
                                                    (&issue.description)
                                                    if !issue.attribute.is_empty() {
                                                        " " <span class="text-muted-foreground">"Field: " <code data-merchant-field="">(&issue.attribute)</code></span>
                                                    }
                                                    if !issue.detail.is_empty() {
                                                        <span class="block text-muted-foreground">(&issue.detail)</span>
                                                    }
                                                    if !issue.documentation.is_empty() {
                                                        <a href=(&issue.documentation) class=(LINK) rel="noreferrer">"What Google says"</a>
                                                    }
                                                </li>
                                            }
                                        </ul>
                                    )
                                )
                            }
                        )
                    )
                )
            }
        )
    })
}
