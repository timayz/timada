//! `/research`: the niche studies a user keeps. A study is a candidate
//! niche, looked at before any shop exists: a name, the seed keywords it
//! starts from, the country and the language they are searched in, and the
//! keywords the platform's keyword source gave for each seed. It belongs to
//! the user who made it, and nobody else opens it.

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
use crate::ui::{
    LINK, Shell, document, empty_state, format_date, icons, notice, page_header, section_card,
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
                                table_head("Searched in")
                                table_head("Seeds")
                                table_head("Keywords")
                                table_head("Started")
                            )
                        )
                        table_body(
                            for row in &mine {
                                table_row(
                                    attrs: attributes! { data-study=(&row.name) },
                                    table_cell(
                                        <a class=(LINK) href=(href!(study_id::index, study_id::study_id: row.id.clone()))>(&row.name)</a>
                                    )
                                    table_cell(<code class="text-xs">(&row.country) " · " (&row.language)</code>)
                                    table_cell(attrs: attributes! { class="tabular-nums" }, (row.seed_count))
                                    table_cell(attrs: attributes! { class="tabular-nums" data-study-keywords=(row.keyword_count) }, (row.keyword_count))
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
