//! `/t/{slug}/newsletter/campaigns/{campaign_id}/schedule`, for members with
//! `newsletter:manage`: when a campaign goes out, rather than at once. A
//! preset (once, every day, every week on some days, every month on a day),
//! a window of the day and a pace: the mails are spread evenly over the
//! window's slots, each at its hour in the subscriber's own time zone. Set
//! on a draft, it is started from the send page; set on a scheduled
//! campaign, it changes the editions to come. A scheduled campaign is
//! stopped from here too.

use std::collections::HashMap;

use jiff::tz::TimeZone;
use timada_newsletter::schedule::{
    Cadence, MAX_MONTH_DAY, PACES, Schedule, format_time_of_day, parse_day, parse_time_of_day,
    weekday_name,
};
use timada_newsletter::{CampaignState, CampaignStatus, count_audience};
use timada_tenancy::PERMISSION_NEWSLETTER_MANAGE;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, route,
    },
    view::{View, attributes, view},
};

use super::{CampaignId, campaign_url, require_campaign};
use crate::app::t::slug::{Slug, TenantNav, require_tenant};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::checkbox::checkbox;
use crate::components::field::{field, field_description, field_label};
use crate::components::input::input;
use crate::components::select::select;
use crate::db::Db;
use crate::ui::{Shell, document, page_header, section_card};

/// The shop's day now.
pub fn shop_today(shop_zone: &str) -> jiff::civil::Date {
    jiff::Timestamp::now()
        .to_zoned(TimeZone::get(shop_zone).unwrap_or(TimeZone::UTC))
        .date()
}

/// A day as the pages show it: `Tue 3 Nov 2026`.
pub fn format_day(day: jiff::civil::Date) -> String {
    day.strftime("%a %-d %b %Y").to_string()
}

/// A list in words: `a`, `a and b`, `a, b and c`.
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// When a schedule sends, in words.
pub fn describe(plan: &Schedule, shop_zone: &str) -> String {
    let days: Vec<String> = plan
        .weekdays
        .iter()
        .map(|day| format!("{}s", weekday_name(*day)))
        .collect();
    let when = match plan.cadence {
        Cadence::Once => {
            let days: Vec<String> = plan
                .first_cycle()
                .map(|cycle| plan.days(cycle))
                .unwrap_or_default()
                .into_iter()
                .map(format_day)
                .collect();
            format!("Once, on {}", and_list(&days))
        }
        Cadence::Daily => "Every day".to_owned(),
        Cadence::Weekly => format!("Every week on {}", and_list(&days)),
        Cadence::Monthly => format!("Every month on day {}", plan.month_day.unwrap_or_default()),
    };
    let hours = match plan.window_end {
        Some(end) => format!(
            "from {} to {}, a batch every {} min",
            format_time_of_day(plan.window_start),
            format_time_of_day(end),
            plan.pace_minutes
        ),
        None => format!("at {}", format_time_of_day(plan.window_start)),
    };
    let span = match (plan.is_recurring(), plan.ends_on) {
        (true, Some(ends_on)) => format!(
            ", from {} until {}",
            format_day(plan.starts_on),
            format_day(ends_on)
        ),
        (true, None) => format!(", from {}", format_day(plan.starts_on)),
        (false, _) => String::new(),
    };
    format!(
        "{when}{span}, {hours}, in each subscriber's time zone ({shop_zone} for those whose zone is unknown)."
    )
}

/// How an edition spreads over its slots for `audience` subscribers.
pub fn pacing(plan: &Schedule, audience: u64) -> String {
    let slots = plan
        .first_cycle()
        .map(|cycle| plan.slots(cycle).len())
        .unwrap_or_default()
        .max(1) as u64;
    if slots == 1 {
        return format!("All {audience} subscriber(s) at once.");
    }
    format!(
        "{slots} slots per edition: about {} mail(s) each for the {audience} subscriber(s) of today.",
        audience.div_ceil(slots)
    )
}

/// The schedule's next days, from the shop's today.
pub fn upcoming(plan: &Schedule, today: jiff::civil::Date) -> Vec<String> {
    plan.upcoming_days(today, 3)
        .into_iter()
        .map(format_day)
        .collect()
}

/// A form value, trimmed; blank is none.
fn value<'a>(form: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    form.get(name)
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
}

/// The schedule the form describes; `None` when a field does not read.
fn schedule_of(form: &HashMap<String, String>) -> Option<Schedule> {
    let cadence = Cadence::parse(value(form, "cadence")?)?;
    let weekdays = (1..=7u8)
        .filter(|day| value(form, &format!("weekday_{day}")).is_some())
        .collect();
    let month_day = match value(form, "month_day") {
        Some(day) => Some(day.parse().ok()?),
        None => None,
    };
    let ends_on = match value(form, "ends_on") {
        Some(day) if cadence != Cadence::Once => Some(parse_day(day)?),
        _ => None,
    };
    let window_end = match value(form, "window_end") {
        Some(time) => Some(parse_time_of_day(time)?),
        None => None,
    };
    Some(Schedule {
        cadence,
        weekdays,
        month_day: (cadence == Cadence::Monthly).then_some(month_day).flatten(),
        starts_on: parse_day(value(form, "starts_on")?)?,
        ends_on,
        window_start: parse_time_of_day(value(form, "window_start")?)?,
        window_end,
        pace_minutes: value(form, "pace").and_then(|pace| pace.parse().ok())?,
    })
}

/// What the form shows before anything is set: next week, Tuesdays and
/// Thursdays, 9:00 to 17:00, every 30 minutes.
fn suggested(today: jiff::civil::Date) -> Schedule {
    Schedule {
        cadence: Cadence::Weekly,
        weekdays: vec![2, 4],
        month_day: Some(1),
        starts_on: today,
        ends_on: None,
        window_start: 9 * 60,
        window_end: Some(17 * 60),
        pace_minutes: 30,
    }
}

#[page]
pub async fn schedule(cx: &Cx) -> Result<impl View> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let campaign = require_campaign(&access, cx).await?;
    let slug = access.tenant.slug.clone();
    if !campaign.is_editable() {
        return Err(see_other(campaign_url(cx, &slug, &campaign.id, None, None)).into());
    }
    let audience = count_audience(
        app_context::<Db>(cx),
        &access.tenant.id,
        campaign.language.as_deref(),
    )
    .await
    .map_err(topcoat::Error::from_anyhow)?;
    let nav = TenantNav::of(cx, &access).await?;
    let tenant = access.tenant;
    let shop_zone = tenant.shop_time_zone().to_owned();
    let today = shop_today(&shop_zone);
    let shown = campaign
        .schedule
        .clone()
        .unwrap_or_else(|| suggested(today));
    let title = format!(
        "Schedule · {} · Newsletter · {}",
        campaign.subject, tenant.name
    );
    let back = campaign_url(cx, &slug, &campaign.id, None, None);
    let save_url = href!(save, Slug(slug.clone()), CampaignId(campaign.id.clone())).resolve(cx);
    let paces = PACES;
    Ok(view! {
        document(
            title: &title,
            shell: Shell::tenant(&tenant, &nav, "newsletter"),
            page_header(
                title: "Schedule the campaign",
                subtitle: Some(campaign.subject.as_str()),
                <a class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm)) href=(&back)>"Back to the campaign"</a>
            )
            if let Some(current) = &campaign.schedule {
                section_card(
                    title: Some("Now"),
                    <p class="text-sm" data-schedule-summary="">(describe(current, &shop_zone))</p>
                    <p class="mt-1 text-sm text-muted-foreground" data-schedule-pacing="">(pacing(current, audience))</p>
                )
            }
            section_card(
                title: Some("When it goes out"),
                description: Some("The mails are spread evenly over the window's slots, at each slot's hour where the subscriber lives."),
                <form method="post" action=(&save_url) class="flex max-w-2xl flex-col gap-5" data-schedule-form="">
                    field(
                        field_label(attrs: attributes! { for="schedule-cadence" }, "Repeat")
                        select(
                            attrs: attributes! { id="schedule-cadence" name="cadence" },
                            <option value="once" selected=(shown.cadence == Cadence::Once)>"Once"</option>
                            <option value="daily" selected=(shown.cadence == Cadence::Daily)>"Every day"</option>
                            <option value="weekly" selected=(shown.cadence == Cadence::Weekly)>"Every week"</option>
                            <option value="monthly" selected=(shown.cadence == Cadence::Monthly)>"Every month"</option>
                        )
                    )
                    <fieldset class="min-w-0">
                        <legend class="text-sm font-medium">"Days"</legend>
                        <p class="text-sm text-muted-foreground">"Every week, or once: the first of these days from the start date. Once without a day is the start date."</p>
                        <div class="mt-2 flex flex-wrap gap-x-4 gap-y-1">
                            for day in 1..=7u8 {
                                <label class="flex items-center gap-2 py-0.5 text-sm" data-weekday=(day)>
                                    checkbox(attrs: attributes! {
                                        name=(format!("weekday_{day}")) value="on"
                                        checked=(shown.weekdays.contains(&day))
                                    })
                                    (weekday_name(day))
                                </label>
                            }
                        </div>
                    </fieldset>
                    field(
                        field_label(attrs: attributes! { for="schedule-month-day" }, "Day of the month")
                        field_description(attrs: attributes! { id="schedule-month-day-hint" }, "Every month only: 1 to 28, a day every month has.")
                        input(attrs: attributes! {
                            id="schedule-month-day" name="month_day" type="number" min="1" max=(MAX_MONTH_DAY)
                            aria-describedby="schedule-month-day-hint"
                            value=(shown.month_day.map(|day| day.to_string()).unwrap_or_default())
                        })
                    )
                    <div class="grid gap-4 sm:grid-cols-2">
                        field(
                            field_label(attrs: attributes! { for="schedule-starts-on" }, "From")
                            input(attrs: attributes! {
                                id="schedule-starts-on" name="starts_on" type="date" required=""
                                value=(shown.starts_on.to_string())
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="schedule-ends-on" }, "Until (optional)")
                            input(attrs: attributes! {
                                id="schedule-ends-on" name="ends_on" type="date"
                                value=(shown.ends_on.map(|day| day.to_string()).unwrap_or_default())
                            })
                        )
                    </div>
                    <div class="grid gap-4 sm:grid-cols-3">
                        field(
                            field_label(attrs: attributes! { for="schedule-window-start" }, "From (local time)")
                            input(attrs: attributes! {
                                id="schedule-window-start" name="window_start" type="time" required=""
                                value=(format_time_of_day(shown.window_start))
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="schedule-window-end" }, "Until (optional)")
                            input(attrs: attributes! {
                                id="schedule-window-end" name="window_end" type="time"
                                value=(shown.window_end.map(format_time_of_day).unwrap_or_default())
                            })
                        )
                        field(
                            field_label(attrs: attributes! { for="schedule-pace" }, "A batch every")
                            select(
                                attrs: attributes! { id="schedule-pace" name="pace" },
                                for pace in paces {
                                    <option value=(pace) selected=(pace == shown.pace_minutes)>
                                        (if pace < 60 { format!("{pace} min") } else { format!("{} h", pace / 60) })
                                    </option>
                                }
                            )
                        )
                    </div>
                    <p class="text-sm text-muted-foreground">
                        "Without an end time, everyone gets it at the start time. Subscribers whose time zone is unknown get it in the shop's (" (&shop_zone) ")."
                    </p>
                    <div class="flex flex-wrap gap-2">
                        button(attrs: attributes! { type="submit" name="action" value="save" }, "Save the schedule")
                        if campaign.schedule.is_some() && campaign.status == CampaignStatus::Draft {
                            button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" name="action" value="clear" formnovalidate="" }, "Send at once instead")
                        }
                    </div>
                </form>
            )
        )
    })
}

/// Saves the schedule; `action=clear` drops it, `action=stop` stops a
/// scheduled campaign.
#[route(POST)]
pub async fn save(cx: &Cx, Form(form): Form<HashMap<String, String>>) -> Result<SeeOther> {
    let access = require_tenant(cx).await?;
    access
        .require_permission(cx, PERMISSION_NEWSLETTER_MANAGE)
        .await?;
    let campaign = require_campaign(&access, cx).await?;
    let slug = access.tenant.slug.clone();
    let newsletter = crate::app::t::slug::newsletter::newsletter(&access)?;
    if value(&form, "action") == Some("stop") {
        return match newsletter
            .stop_schedule(&access.user_id, &campaign.id)
            .await
        {
            Ok(()) => Ok(see_other(campaign_url(
                cx,
                &slug,
                &campaign.id,
                None,
                Some("Schedule stopped: nobody else gets it."),
            ))),
            Err(err) if err.is_user_facing() => Ok(see_other(campaign_url(
                cx,
                &slug,
                &campaign.id,
                err.code(),
                None,
            ))),
            Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
        };
    }
    let (done, message) = if value(&form, "action") == Some("clear") {
        (
            newsletter
                .clear_schedule(&access.user_id, &campaign.id)
                .await,
            "It goes out at once when sent.",
        )
    } else {
        let Some(planned) = schedule_of(&form) else {
            return Ok(see_other(schedule_url(
                cx,
                &slug,
                &campaign,
                timada_newsletter::CAMPAIGN_INVALID_SCHEDULE,
            )));
        };
        let today = shop_today(access.tenant.shop_time_zone());
        (
            newsletter
                .set_schedule(&access.user_id, &campaign.id, planned, today)
                .await,
            "Schedule saved.",
        )
    };
    match done {
        Ok(()) => Ok(see_other(campaign_url(
            cx,
            &slug,
            &campaign.id,
            None,
            Some(message),
        ))),
        Err(err) if err.is_user_facing() => Ok(see_other(schedule_url(
            cx,
            &slug,
            &campaign,
            err.code().unwrap_or("campaign_invalid_schedule"),
        ))),
        Err(err) => Err(topcoat::Error::from_anyhow(err.into())),
    }
}

/// The campaign page with an error to show: the schedule form errors are
/// shown there, beside the schedule it still has.
fn schedule_url(cx: &Cx, slug: &str, campaign: &CampaignState, error: &str) -> String {
    campaign_url(cx, slug, &campaign.id, Some(error), None)
}
