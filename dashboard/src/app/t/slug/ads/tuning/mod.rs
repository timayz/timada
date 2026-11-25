//! `/t/{slug}/ads/tuning`: the tuning rounds of the shop's first campaign,
//! for members with `orders:read`. A round reads the campaign's last weeks
//! and prepares proposals (a bid a step up or down, a product to keep out
//! of ads, a search to exclude, a device's adjustment): the page says what
//! the last one prepared, what it looked at and left alone, and what a
//! person should look at. A member with `ads:manage` asks for a round and
//! sets the thresholds it decides on; nothing is changed at Google Ads
//! before a proposal is approved (`crate::tuning`).

pub mod round;
pub mod set;

use timada_google::{
    CAMPAIGN_SHOPPING, GoogleError, TUNING_DAYS, TUNING_DRAIN_CLICKS, TUNING_EVERY_DAYS,
    TUNING_FIELDS, TUNING_MIN_CLICKS, TUNING_REST_DAYS, TUNING_STEP, TUNING_TARGET_RETURN,
    TUNING_TERM_CLICKS, TUNING_WINNER_CLICKS,
};
use timada_tenancy::{PERMISSION_ADS_MANAGE, PERMISSION_ORDERS_READ};
use topcoat::{
    Result,
    context::Cx,
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use super::super::google::google;
use super::super::{TenantAccess, TenantNav, require_tenant, slug};
use crate::app::auth::Flash;
use crate::auth::now;
use crate::campaigns::held;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::field::{field, field_description, field_label as label};
use crate::components::input::input;
use crate::tuning::may_tune;
use crate::ui::{
    LINK, Shell, document, fact, facts, format_time, icons, notice, page_header, section_card,
};

#[topcoat::router::query_params(error = bad_request)]
struct TuningQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with a flash error code or message.
pub fn tuning_url(cx: &Cx, slug: &str, error: Option<&str>, message: Option<&str>) -> String {
    href!(index, slug: slug.to_owned())
        .query(Flash { error, message })
        .resolve(cx)
}

/// A threshold's name and what its field says under it.
fn threshold(field: &str) -> (&'static str, &'static str) {
    match field {
        TUNING_DAYS => (
            "Days read",
            "How far back a round reads the campaign: 7 to 90 days.",
        ),
        TUNING_MIN_CLICKS => (
            "Clicks before a bid moves",
            "An ad group, or a kind of device, with fewer clicks over those days is left alone, and the round says so.",
        ),
        TUNING_DRAIN_CLICKS => (
            "Clicks without a sale before a product is kept out",
            "A product clicked that often and never sold is proposed for keeping out of ads. It stays in the shop.",
        ),
        TUNING_WINNER_CLICKS => (
            "Clicks before a product is a winner",
            "With that many clicks and a return half again above its break-even, a product is proposed for a campaign of its own.",
        ),
        TUNING_TERM_CLICKS => (
            "Clicks without a sale before a search is excluded",
            "Only a search with no word in common with the products it showed.",
        ),
        TUNING_STEP => (
            "Bid step",
            "What a bid moves by, in the account's currency: one step, or two when the return is far off.",
        ),
        TUNING_TARGET_RETURN => (
            "Target return",
            "What an ad group is held to while the shop's own sales do not say its break-even: 3.00 is 3 sold for 1 spent.",
        ),
        TUNING_REST_DAYS => (
            "Days a change is left to show",
            "A bid just changed is not looked at again before.",
        ),
        TUNING_EVERY_DAYS => (
            "A round every so many days",
            "0 for only when you ask. Otherwise a round is prepared by itself after the daily read, a week apart at least.",
        ),
        _ => ("Threshold", ""),
    }
}

/// One threshold of the form.
struct Threshold {
    field: &'static str,
    label: &'static str,
    hint: &'static str,
    value: String,
}

/// What the last round did, as the page shows it.
struct Last {
    at: u64,
    first: String,
    last: String,
    proposals: usize,
    notes: Vec<String>,
    flags: Vec<String>,
    error: Option<String>,
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_ORDERS_READ)
        .await?;
    let query = query_params::<TuningQuery>(cx)?;
    let nav = TenantNav::of(cx, &access).await?;
    let can_manage = access.can(cx, PERMISSION_ADS_MANAGE).await?;
    let failed = topcoat::Error::from_anyhow;
    let campaign = held(access.executor()?, access.tenant_id(), CAMPAIGN_SHOPPING)
        .await
        .map_err(failed)?;
    // Why a round cannot be asked for yet, for the member.
    let waits = match may_tune(campaign.as_ref(), now()) {
        Ok(()) => None,
        Err(GoogleError::NoCampaign) => {
            Some("The shop has no first campaign yet: there is nothing to tune.".to_owned())
        }
        Err(_) => Some(
            match campaign.as_ref().and_then(|campaign| campaign.settles_at()) {
                Some(until) => format!(
                    "The campaign is in its first three weeks: a round can be asked for from {}.",
                    format_time(until)
                ),
                None => "The campaign does not run yet: set it running first.".to_owned(),
            },
        ),
    };
    let tuning = google(cx)
        .load_tuning(access.tenant_id())
        .await
        .map_err(failed)?;
    let settings = tuning
        .as_ref()
        .map(|tuning| tuning.thresholds())
        .unwrap_or_default();
    let asked = tuning.as_ref().is_some_and(|tuning| tuning.round_requested);
    let last = tuning
        .and_then(|tuning| tuning.last_round)
        .map(|round| Last {
            at: round.at,
            first: round.first,
            last: round.last,
            proposals: round.proposals.len(),
            notes: round.notes,
            flags: round.flags,
            error: round.error,
        });
    let thresholds: Vec<Threshold> = TUNING_FIELDS
        .iter()
        .map(|(field, _)| {
            let (label, hint) = threshold(field);
            Threshold {
                field,
                label,
                hint,
                value: settings.value(field),
            }
        })
        .collect();
    let TenantAccess { tenant, .. } = access;
    let slug = tenant.slug.clone();
    let title = format!("Tuning · Google Ads · {}", tenant.name);
    let campaigns_page = href!(super::campaigns::index, slug: slug.clone()).resolve(cx);
    let proposals_page = href!(super::super::proposals::index, slug: slug.clone()).resolve(cx);
    let round_url = href!(round::round, slug: slug.clone()).resolve(cx);
    let set_url = href!(set::set, slug: slug.clone()).resolve(cx);

    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "ads"),
            page_header(
                title: "Tuning",
                subtitle: Some("A round reads the campaign's last weeks and prepares the changes the same few rules give: you read each with its numbers and approve it. Nothing is changed at Google Ads before."),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(&campaigns_page)>"Campaigns"</a>
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            section_card(
                title: Some("Rounds"),
                attrs: attributes! { data-tuning=(if asked { "asked" } else if waits.is_some() { "waits" } else { "ready" }) },
                if let Some(waits) = &waits {
                    <p class="text-sm text-muted-foreground" data-tuning-waits="">(waits)</p>
                } else if asked {
                    <p class="text-sm" data-tuning-asked="">"A round is being prepared."</p>
                } else if can_manage {
                    <form method="post" action=(&round_url)>
                        button(attrs: attributes! { type="submit" }, "Prepare a round")
                    </form>
                }
                if let Some(last) = &last {
                    <div class="mt-6" data-tuning-last="">
                        if let Some(error) = &last.error {
                            alert(
                                variant: AlertVariant::Destructive,
                                attrs: attributes! { role="alert" data-tuning-error="" },
                                icon(data: icons::WARNING)
                                alert_description("The last round was not prepared. " (error))
                            )
                        } else {
                            facts(
                                fact(term: "Last round", <time datetime=(last.at)>(format_time(last.at))</time>)
                                fact(term: "Days read", (&last.first) " to " (&last.last))
                                fact(
                                    term: "Prepared",
                                    attrs: attributes! { data-tuning-proposals=(last.proposals) },
                                    if last.proposals == 0 {
                                        "Nothing to change."
                                    } else {
                                        (last.proposals) " changes, under "
                                        <a class=(LINK) href=(&proposals_page)>"Proposals"</a>
                                    }
                                )
                            )
                            if !last.flags.is_empty() {
                                <h3 class="mt-4 text-sm font-medium">"To look at"</h3>
                                <ul class="mt-2 flex list-disc flex-col gap-1 pl-5 text-sm" data-tuning-flags="">
                                    for flag in &last.flags {
                                        <li>(flag)</li>
                                    }
                                </ul>
                            }
                            if !last.notes.is_empty() {
                                <h3 class="mt-4 text-sm font-medium">"Left alone"</h3>
                                <ul class="mt-2 flex list-disc flex-col gap-1 pl-5 text-sm text-muted-foreground" data-tuning-notes="">
                                    for note in &last.notes {
                                        <li>(note)</li>
                                    }
                                </ul>
                            }
                        }
                    </div>
                }
            )

            section_card(
                title: Some("Thresholds"),
                description: Some("What a round decides on. They come from one method and one kind of shop: change them once your own numbers say otherwise."),
                <form method="post" action=(&set_url) data-tuning-thresholds="">
                    <fieldset disabled=(!can_manage) class="flex max-w-3xl flex-col gap-4">
                        for threshold in &thresholds {
                            field(
                                label(attrs: attributes! { for=(format!("tuning-{}", threshold.field)) }, (threshold.label))
                                field_description(attrs: attributes! { id=(format!("tuning-{}-hint", threshold.field)) }, (threshold.hint))
                                input(attrs: attributes! { id=(format!("tuning-{}", threshold.field)) name=(threshold.field) type="text" inputmode="decimal" value=(&threshold.value) aria-describedby=(format!("tuning-{}-hint", threshold.field)) class="w-40" })
                            )
                        }
                        if can_manage {
                            <div>button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Save the thresholds")</div>
                        }
                    </fieldset>
                </form>
            )
        )
    })
}
