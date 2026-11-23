//! `/admin/research`: what the keyword source was paid today and this month
//! (UTC), by user, beside the day's caps. The figures come from the shared
//! read model, a moment behind the calls; what a cap decides on is the
//! day's own stream.

use std::collections::HashMap;

use timada_identity::users_by_ids;
use timada_research::{spend_by_user, spend_day, spend_in_all};
use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page},
    view::{View, attributes, view},
};

use crate::app::admin::{require_admin, users::user_id};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::config::Config;
use crate::db::Db;
use crate::keywords::Keywords;
use crate::research::format_usd;
use crate::ui::{LINK, Shell, document, empty_row, fact, facts, page_header, section_card};

/// The page lists this many users at most, the ones who spent the most in
/// the month.
const LISTED: i64 = 200;

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let db = context::<Db>(cx);
    let config = &context::<Config>(cx).research;
    let caps = config.caps();
    let enabled = context::<Keywords>(cx).enabled();
    let day = spend_day(crate::auth::now());
    let rows = spend_by_user(db, &day, LISTED)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let (day_total, month_total) = spend_in_all(db, &day)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let ids: Vec<String> = rows.iter().map(|row| row.user_id.clone()).collect();
    let users: HashMap<String, (String, String)> = users_by_ids(db, &ids)
        .await
        .map_err(topcoat::Error::from_anyhow)?
        .into_iter()
        .map(|user| (user.id, (user.label, user.account)))
        .collect();
    let month = &day[..7];
    let subtitle = format!(
        "What the keyword source was paid on {day} and in {month} so far (UTC). Every call is paid by the platform's account."
    );
    let reuse = match config.reuse_days {
        0 => "Never: every fetch is a call".to_owned(),
        1 => "For a day".to_owned(),
        days => format!("For {days} days"),
    };
    let usd = |micros: i64| format_usd(micros.max(0) as u64);

    Ok(view! {
        document(
            title: "Research spend",
            shell: Shell::page("admin_research"),
            page_header(title: "Research spend", subtitle: Some(&subtitle))

            section_card(
                title: Some("The day and the month"),
                facts(
                    fact(term: "Keyword source", if enabled { "Set" } else { "Not set: keyword research is off" })
                    fact(term: "Spent today", attrs: attributes! { data-spend-day=(day_total) },
                        (usd(day_total)) " of " (format_usd(caps.platform_usd_micros)) " a day"
                    )
                    fact(term: "Spent this month", attrs: attributes! { data-spend-month=(month_total) }, (usd(month_total)))
                    fact(term: "A user's cap", (format_usd(caps.user_usd_micros)) " a day")
                    fact(term: "A seed's keywords are reused", (&reuse))
                )
            )

            section_card(
                title: Some("By user"),
                description: Some("The users whose fetches were paid for this month, the one who spent the most first. A fetch answered from keywords already fetched costs nothing and is not counted."),
                table(
                    table_header(
                        table_row(
                            table_head("User")
                            table_head("Today")
                            table_head("Calls today")
                            table_head("This month")
                            table_head("Calls this month")
                        )
                    )
                    table_body(
                        if rows.is_empty() {
                            empty_row(colspan: 5, text: "Nothing was spent this month.")
                        }
                        for row in &rows {
                            table_row(
                                attrs: attributes! { data-spend-user=(&row.user_id) },
                                table_cell(
                                    if let Some((label, account)) = users.get(&row.user_id) {
                                        <a class=(LINK) href=(href!(user_id::user, user_id::user_id: row.user_id.clone()))>(label)</a>
                                        <span class="block text-xs text-muted-foreground">(account)</span>
                                    } else {
                                        (&row.user_id)
                                    }
                                )
                                table_cell(attrs: attributes! { class="tabular-nums" data-spend-user-day=(row.day_usd_micros) }, (usd(row.day_usd_micros)))
                                table_cell(attrs: attributes! { class="tabular-nums" }, (row.day_calls))
                                table_cell(attrs: attributes! { class="tabular-nums" data-spend-user-month=(row.month_usd_micros) }, (usd(row.month_usd_micros)))
                                table_cell(attrs: attributes! { class="tabular-nums" }, (row.month_calls))
                            )
                        }
                    )
                )
            )
        )
    })
}
