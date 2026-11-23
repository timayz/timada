//! `/research`: the niche studies a user keeps. A study is a candidate
//! niche, looked at before any shop exists: a name, the seed keywords it
//! starts from, the country and the language they are searched in, and the
//! keywords the platform's keyword source gave for each seed. It belongs to
//! the user who made it, and nobody else opens it. The list sets a user's
//! studies side by side, each with its verdict and the numbers three or
//! four candidates are compared by before one is chosen.

pub mod new;
pub mod study_id;

use timada_research::{Command, ResearchError, studies_of_user};
use topcoat::{
    Result,
    context::{Cx, context},
    icon::icon,
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::auth::Flash;
use crate::auth::require_user;
use crate::cluster::Executor;
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonSize, ButtonVariant, button_variants};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::db::Db;
use crate::keywords::Keywords;
use crate::research::format_ratio;
use crate::ui::{
    LINK, Shell, Tone, document, empty_state, format_date, icons, notice, page_header,
    section_card, status_badge,
};

/// The list shows this many studies: a user compares a handful.
const LISTED: i64 = 200;

#[topcoat::router::query_params(error = bad_request)]
struct ResearchQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn research_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(index).query(Flash { error, message }).resolve(cx)
}

/// The research command gateway over this node's platform store.
pub fn research(cx: &Cx) -> Command<Executor> {
    Command(context::<Executor>(cx).clone())
}

/// The query-string code the pages show for a domain error.
pub fn error_code(err: &ResearchError) -> &'static str {
    err.code().unwrap_or("failed")
}

/// How a stored verdict reads.
fn verdict_label(verdict: &str) -> (&'static str, Tone) {
    match verdict {
        "go" => ("Go", Tone::Success),
        "no_go" => ("No-go", Tone::Danger),
        _ => ("Incomplete", Tone::Neutral),
    }
}

/// A figure in hundredths the score may have nothing for.
fn ratio(hundredths: Option<i64>) -> String {
    hundredths.map_or_else(|| "–".to_owned(), |value| format_ratio(value as u64))
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let query = query_params::<ResearchQuery>(cx)?;
    let mine = studies_of_user(context::<Db>(cx), &user.id, LISTED)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let enabled = context::<Keywords>(cx).enabled();
    Ok(view! {
        document(
            title: "Research",
            shell: Shell::page("research"),
            page_header(
                title: "Research",
                subtitle: Some("A study is a niche you are looking at before opening a shop for it: the keywords people search it by, how often, what a click costs and how hard the first page is to reach."),
                if enabled {
                    <a class=(button_variants(ButtonVariant::Primary, ButtonSize::Md)) href=(href!(new::new))>
                        icon(data: icons::PLUS)
                        "Start a study"
                    </a>
                }
            )
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            if !enabled {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-4" data-research-disabled="" },
                    icon(data: icons::WARNING)
                    alert_description("Keyword research is not set up on this platform: the operator has to give it a keyword source.")
                )
            }
            section_card(
                title: Some("Your studies"),
                description: Some("Side by side, as each was last scored: the verdict, the kept keywords, the return on ad spend at which an ad breaks even, and the sale's price over the cost of a click against what it needs."),
                if mine.is_empty() {
                    empty_state(
                        title: "No study yet",
                        hint: Some("Start one from a few keywords a shopper would type, then fetch what is searched around them."),
                        symbol: Some(icons::RESEARCH),
                    )
                } else {
                    table(
                        table_header(
                            table_row(
                                table_head("Study")
                                table_head("Verdict")
                                table_head("Main")
                                table_head("Long tail")
                                table_head("Kept searches a month")
                                table_head("Break-even")
                                table_head("Price / CPC")
                                table_head("Needed")
                                table_head("Keywords")
                                table_head("Searched in")
                                table_head("Started")
                            )
                        )
                        table_body(
                            for row in &mine {
                                table_row(
                                    attrs: attributes! { data-study=(&row.name) data-study-verdict=(&row.verdict) },
                                    table_cell(
                                        <a class=(LINK) href=(href!(study_id::index, study_id::study_id: row.id.clone()))>(&row.name)</a>
                                    )
                                    table_cell(status_badge(tone: verdict_label(&row.verdict).1, (verdict_label(&row.verdict).0)))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-main="" }, (row.main_count))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-long-tail="" }, (row.long_tail_count))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-volume="" }, (row.kept_volume))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-break-even="" }, (ratio(row.break_even_roas)))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-ratio="" }, (ratio(row.price_per_click)))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-needed="" }, (ratio(row.needed_price_per_click)))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-keywords=(row.keyword_count) }, (row.keyword_count))
                                    table_cell(<code class="text-xs">(&row.country) " · " (&row.language)</code>)
                                    table_cell(<time datetime=(format_date(row.created_at as u64))>(format_date(row.created_at as u64))</time>)
                                )
                            }
                        )
                    )
                }
            )
        )
    })
}
