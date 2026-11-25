//! `/t/{slug}/ads/campaigns`: the campaigns timada creates in the shop's
//! Google Ads account, for members with `orders:read`: the first Shopping
//! campaign, an ad group a category, and a small Search campaign on the
//! shop's own name. Each says what it still waits for, and a member with
//! `ads:manage` fills its settings and prepares it as a proposal: nothing
//! is sent to Google Ads before a member read the whole of it and
//! approved (`crate::campaigns`). Once created, the page says whether it
//! runs and since when, and to leave it alone for its first three weeks.

pub mod live;
pub mod prepare;

use timada_google::{
    CAMPAIGN_AD_GROUPS, CAMPAIGN_BID, CAMPAIGN_COUNTRIES, CAMPAIGN_DAILY_BUDGET,
    CAMPAIGN_DESCRIPTIONS, CAMPAIGN_HEADLINES, CAMPAIGN_KEYWORDS, CAMPAIGN_KINDS, CAMPAIGN_NAME,
    CAMPAIGN_PHONE, CAMPAIGN_SHOPPING, CAMPAIGN_START, CAMPAIGN_TABLET, MAX_CAMPAIGN_AD_GROUPS,
    SETTLE_DAYS, START_LIVE, START_PAUSED,
};
use timada_proposals::{KIND_ADS_CAMPAIGN, waiting_about};
use timada_tenancy::{PERMISSION_ADS_MANAGE, PERMISSION_ORDERS_READ};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::super::proposals::proposal_id;
use super::super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::auth::now;
use crate::campaigns::{Check, field_label, held, is_long, kind_label, ready};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label as label};
use crate::components::input::input;
use crate::components::select::select;
use crate::components::textarea::textarea;
use crate::db::Db;
use crate::ui::{
    LINK, Shell, Tone, document, fact, facts, format_time, icons, notice, page_header,
    section_card, status_badge,
};

#[topcoat::router::query_params(error = bad_request)]
struct CampaignsQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn campaigns_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// One setting of the form that prepares a campaign.
struct Setting {
    field: &'static str,
    label: &'static str,
    value: String,
    hint: String,
    long: bool,
}

/// A campaign Google Ads holds.
struct Created {
    name: String,
    /// Google Ads' number for it.
    number: String,
    created_at: u64,
    ad_groups: usize,
    /// The proposal it was approved through.
    proposal: String,
    live_at: Option<u64>,
    /// Unix seconds until when it is left alone; 0 once that is over, or
    /// while it is paused.
    settling_until: u64,
    /// A member asked for it to run and Google Ads was not told yet.
    asked: bool,
    live_error: Option<String>,
}

/// One kind of campaign as the page shows it.
struct Card {
    kind: &'static str,
    title: &'static str,
    about: &'static str,
    created: Option<Created>,
    /// A proposal that waits for an answer, in place of the form.
    waiting: Option<String>,
    checks: Vec<Check>,
    ready: bool,
    settings: Vec<Setting>,
    /// What the form leaves out, for the member to know.
    notes: Vec<String>,
}

/// What a setting's field says under it.
fn hint_of(field: &str, currency: &str) -> String {
    match field {
        CAMPAIGN_NAME => {
            "What Google Ads calls it. No other campaign of the account may bear the name."
                .to_owned()
        }
        CAMPAIGN_DAILY_BUDGET => {
            format!("In {currency}: what the campaign may spend in a day, on average.")
        }
        CAMPAIGN_BID => format!("In {currency}: the most a click is paid, to start with."),
        CAMPAIGN_COUNTRIES => {
            "Where the ads show: the two letters of each country, separated by commas.".to_owned()
        }
        CAMPAIGN_PHONE | CAMPAIGN_TABLET => {
            "How much less (-55) or more (20) a click there is paid than on a computer. Empty for the same."
                .to_owned()
        }
        CAMPAIGN_START => {
            "Paused, it shows nothing until you set it running, here or at Google Ads.".to_owned()
        }
        CAMPAIGN_AD_GROUPS => {
            "One ad group a line: the category's id, then the name the ad group takes. Remove a line to leave a category out."
                .to_owned()
        }
        CAMPAIGN_KEYWORDS => {
            "The searches the ad shows for, a line each: the shop's name as people type it."
                .to_owned()
        }
        CAMPAIGN_HEADLINES => {
            "Three to fifteen, a line each, of 30 characters at most. Google shows two or three at a time."
                .to_owned()
        }
        CAMPAIGN_DESCRIPTIONS => {
            "Two to four, a line each, of 90 characters at most.".to_owned()
        }
        _ => String::new(),
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<CampaignsQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_ADS_MANAGE).await?;
    let store = access.executor()?.clone();
    let db = context::<Db>(cx);
    let failed = topcoat::Error::from_anyhow;
    let now = now();
    let mut cards = Vec::with_capacity(CAMPAIGN_KINDS.len());
    for kind in CAMPAIGN_KINDS {
        let campaign = held(&store, access.tenant_id(), kind)
            .await
            .map_err(failed)?;
        let created = campaign.map(|campaign| Created {
            settling_until: campaign
                .settles_at()
                .filter(|until| now < *until)
                .unwrap_or(0),
            name: campaign.name,
            number: campaign.campaign_id,
            created_at: campaign.created_at,
            ad_groups: campaign.ad_groups.len(),
            proposal: campaign.proposal_id,
            live_at: campaign.live_at,
            asked: campaign.live_requested,
            live_error: campaign.live_error,
        });
        let mut card = Card {
            kind,
            title: kind_label(kind),
            about: match kind {
                CAMPAIGN_SHOPPING => {
                    "Standard Shopping with manual bids: one ad group a category, told by the feed's label, the categories kept out of ads left out."
                }
                _ => {
                    "A small Search campaign on the shop's own name, so that whoever looks for the shop finds it first."
                }
            },
            created,
            waiting: None,
            checks: Vec::new(),
            ready: false,
            settings: Vec::new(),
            notes: Vec::new(),
        };
        if card.created.is_none() {
            card.waiting = waiting_about(db, access.tenant_id(), KIND_ADS_CAMPAIGN, kind)
                .await
                .map_err(failed)?;
            let ready = ready(&store, db, &access.tenant, kind)
                .await
                .map_err(failed)?;
            card.ready = ready.is_ready();
            if !ready.left_out.is_empty() {
                card.notes.push(format!(
                    "Kept out of ads, so left out: {}.",
                    ready.left_out.join(", ")
                ));
            }
            if ready.over > 0 {
                card.notes.push(format!(
                    "A campaign is created with {MAX_CAMPAIGN_AD_GROUPS} ad groups at most: {} more categories are left for you to add at Google Ads.",
                    ready.over
                ));
            }
            card.settings = ready
                .defaults
                .iter()
                .map(|(field, value)| Setting {
                    field,
                    label: field_label(field),
                    value: value.clone(),
                    hint: hint_of(field, &ready.currency),
                    long: is_long(field),
                })
                .collect();
            card.checks = ready.checks;
        }
        cards.push(card);
    }
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Campaigns · Google Ads · {}", tenant.name);
    let ads_page = href!(super::index, slug: slug.clone()).resolve(cx);
    let prepare_url = href!(prepare::prepare, slug: slug.clone()).resolve(cx);
    let live_url = href!(live::live, slug: slug.clone()).resolve(cx);
    let settle_note = format!(
        "Leave it as it is for its first {SETTLE_DAYS} days: a change made on a few days of numbers is a guess."
    );

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "ads"),
            page_header(
                title: "Campaigns",
                subtitle: Some("What timada creates for you in Google Ads. You fill the settings, read the whole campaign as a proposal and approve it: nothing is sent to Google Ads before."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(&ads_page)>"Back to Google Ads"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            for card in &cards {
                section_card(
                    title: Some(card.title),
                    description: Some(card.about),
                    attrs: attributes! { data-campaign=(card.kind) },
                    if let Some(created) = &card.created {
                        facts(
                            fact(term: "Campaign", attrs: attributes! { data-campaign-name="" }, (&created.name) " · number " (&created.number))
                            fact(
                                term: "Created",
                                <time datetime=(created.created_at)>(format_time(created.created_at))</time>
                                " · "
                                <a class=(LINK) href=(href!(proposal_id::index, slug: slug.clone(), proposal_id::proposal_id: created.proposal.clone()))>"as approved"</a>
                            )
                            fact(term: "Ad groups", (created.ad_groups))
                            fact(
                                term: "Status",
                                if let Some(live_at) = created.live_at {
                                    status_badge(tone: Tone::Success, attrs: attributes! { data-campaign-status="live" }, "Live")
                                    " since " <time datetime=(live_at)>(format_time(live_at))</time>
                                } else if created.asked {
                                    status_badge(tone: Tone::Info, attrs: attributes! { data-campaign-status="asked" }, "Being set running")
                                } else {
                                    status_badge(tone: Tone::Neutral, attrs: attributes! { data-campaign-status="paused" }, "Paused")
                                }
                            )
                        )
                        if let Some(error) = &created.live_error {
                            alert(
                                variant: AlertVariant::Destructive,
                                attrs: attributes! { role="alert" class="mt-4" data-campaign-live-error="" },
                                icon(data: icons::WARNING)
                                alert_description("It was not set running. " (error))
                            )
                        }
                        if created.settling_until > 0 {
                            alert(
                                attrs: attributes! { class="mt-4" data-campaign-settling="" },
                                icon(data: icons::INFO)
                                alert_description(
                                    (&settle_note) " Until "
                                    <time datetime=(created.settling_until)>(format_time(created.settling_until))</time> "."
                                )
                            )
                        }
                        if created.live_at.is_none() && !created.asked && can_manage {
                            <form method="post" action=(&live_url) class="mt-4">
                                <input type="hidden" name="kind" value=(card.kind) />
                                button(size: ButtonSize::Sm, attrs: attributes! { type="submit" }, "Set it running")
                            </form>
                            <p class="mt-2 text-sm text-muted-foreground">"Setting it running at Google Ads does as well: the daily read notices the first day it showed an ad."</p>
                        }
                    } else {
                        <ul class="flex flex-col gap-2 text-sm" data-campaign-checks="">
                            for check in &card.checks {
                                <li class="flex items-start gap-2" data-campaign-check=(check.key) data-met=(check.met.to_string())>
                                    if check.met {
                                        status_badge(tone: Tone::Success, "Ready")
                                    } else {
                                        status_badge(tone: Tone::Warning, "Missing")
                                    }
                                    <span>
                                        (check.label)
                                        if !check.met {
                                            <span class="block text-muted-foreground">(&check.missing)</span>
                                        }
                                    </span>
                                </li>
                            }
                        </ul>
                        if let Some(waiting) = &card.waiting {
                            <p class="mt-4 text-sm" data-campaign-waiting="">
                                "It is prepared and waits for an answer: "
                                <a class=(LINK) href=(href!(proposal_id::index, slug: slug.clone(), proposal_id::proposal_id: waiting.clone()))>"read the proposal"</a> "."
                            </p>
                        } else if !card.ready {
                            <p class="mt-4 text-sm text-muted-foreground" data-campaign-not-ready="">"The campaign is prepared once nothing is missing."</p>
                        } else if !can_manage {
                            <p class="mt-4 text-sm text-muted-foreground">"Preparing it needs the permission to manage the ads."</p>
                        } else {
                            <form method="post" action=(&prepare_url) class="mt-6 flex max-w-3xl flex-col gap-4" data-campaign-form="">
                                <input type="hidden" name="kind" value=(card.kind) />
                                for setting in &card.settings {
                                    field(
                                        label(attrs: attributes! { for=(format!("{}-{}", card.kind, setting.field)) }, (setting.label))
                                        field_description(attrs: attributes! { id=(format!("{}-{}-hint", card.kind, setting.field)) }, (&setting.hint))
                                        if setting.field == CAMPAIGN_START {
                                            select(
                                                attrs: attributes! { id=(format!("{}-{}", card.kind, setting.field)) name=(setting.field) aria-describedby=(format!("{}-{}-hint", card.kind, setting.field)) class="w-48" },
                                                <option value=(START_PAUSED) selected=(setting.value == START_PAUSED)>"Paused"</option>
                                                <option value=(START_LIVE) selected=(setting.value == START_LIVE)>"Live"</option>
                                            )
                                        } else if setting.long {
                                            textarea(
                                                attrs: attributes! { id=(format!("{}-{}", card.kind, setting.field)) name=(setting.field) rows="6" aria-describedby=(format!("{}-{}-hint", card.kind, setting.field)) },
                                                (&setting.value)
                                            )
                                        } else {
                                            input(attrs: attributes! { id=(format!("{}-{}", card.kind, setting.field)) name=(setting.field) type="text" value=(&setting.value) aria-describedby=(format!("{}-{}-hint", card.kind, setting.field)) })
                                        }
                                    )
                                }
                                for note in &card.notes {
                                    <p class="text-sm text-muted-foreground" data-campaign-note="">(note)</p>
                                }
                                <div>button(attrs: attributes! { type="submit" }, "Prepare the campaign")</div>
                            </form>
                        }
                    }
                )
            }
        )
    })
}

/// Whether `kind` is a campaign the page prepares; its name as kept.
pub(super) fn known(kind: &str) -> Option<&'static str> {
    CAMPAIGN_KINDS.into_iter().find(|known| *known == kind)
}
