//! `/{mount}/journal`: what operators did, and tried to — every request that
//! wrote, every refusal, every sign-in — the newest first. The owners' page:
//! no section of [`Role`](crate::auth::Role) claims it.

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params, query_params as query},
    view::{View, view},
};

use crate::{
    auth::{
        Section,
        journal::{JournalEntry, JournalFilter, Outcome, SIGN_IN, count_journal, list_journal},
        team::list_operators,
    },
    components::{
        select::select,
        table::{table, table_body, table_cell, table_head, table_header, table_row},
    },
    config::AdminServices,
    ui::{empty_state, field, filter_bar, page_header, pagination, table_card},
};

pub const PAGE_SIZE: u32 = 50;

/// `?operateur=<id>&issue=passed|refused|error&page=N`.
#[query_params(error = bad_request)]
struct JournalQuery {
    operateur: Option<String>,
    issue: Option<String>,
    page: Option<u32>,
}

struct Line {
    when: String,
    who: String,
    role: &'static str,
    place: String,
    action: String,
    target: String,
    outcome: &'static str,
    refused: bool,
    status: u16,
}

/// Where it happened, in the navigation's words.
fn place(entry: &JournalEntry) -> String {
    let segment = entry.section();
    match Section::of_segment(segment) {
        Some(section) => section.label().to_owned(),
        None => match segment {
            SIGN_IN => "Connexion".to_owned(),
            "team" => "Équipe".to_owned(),
            "password" => "Mot de passe".to_owned(),
            "journal" => "Journal".to_owned(),
            other => other.to_owned(),
        },
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    let db = &app_context::<AdminServices>(cx).db;
    let asked = query::<JournalQuery>(cx)?;
    let page_number = asked.page.unwrap_or(1).max(1);
    let filter = JournalFilter {
        admin_id: asked.operateur.clone().filter(|id| !id.is_empty()),
        outcome: asked.issue.as_deref().and_then(Outcome::parse),
        limit: PAGE_SIZE,
        offset: (page_number - 1) * PAGE_SIZE,
    };
    let total = count_journal(db, &filter).await?.max(0) as u64;
    let lines: Vec<Line> = list_journal(db, &filter)
        .await?
        .into_iter()
        .map(|entry| Line {
            when: timada_core::format::date_time(entry.at.max(0) as u64),
            place: place(&entry),
            action: format!("{} {}", entry.method, entry.action())
                .trim()
                .to_owned(),
            target: entry.target().unwrap_or_default().to_owned(),
            role: entry.role.map_or("—", |role| role.label()),
            outcome: entry.outcome.label(),
            refused: entry.outcome != Outcome::Passed,
            status: entry.status,
            who: entry.email,
        })
        .collect();
    // The filters: every operator who is or was one, every outcome.
    let operators: Vec<(String, String, bool)> = match list_operators(db).await {
        Ok(operators) => operators
            .into_iter()
            .map(|operator| {
                let chosen = filter.admin_id.as_deref() == Some(operator.id.as_str());
                (operator.id, operator.email, chosen)
            })
            .collect(),
        Err(err) => return Err(anyhow::Error::from(err).into()),
    };
    let outcomes: Vec<(&'static str, &'static str, bool)> = Outcome::ALL
        .into_iter()
        .map(|outcome| {
            (
                outcome.as_str(),
                outcome.label(),
                filter.outcome == Some(outcome),
            )
        })
        .collect();
    let here = href!(index).resolve(cx);

    Ok(view! {
        page_header(title: "Journal")
        <p class="-mt-4 mb-6 text-sm text-muted-foreground">"Ce que les opérateurs ont fait et tenté : chaque écriture, chaque refus, chaque connexion. Heures UTC. Le contenu des formulaires n'est jamais conservé."</p>
        filter_bar(
            action: here,
            class: "mb-6",
            field(
                label: "Opérateur",
                control: "operateur",
                select(
                    attrs: topcoat::view::attributes! { id="operateur" name="operateur" },
                    <option value="">"Tous"</option>
                    for (id, email, chosen) in &operators {
                        <option value=(id.clone()) selected=(*chosen)>(email.clone())</option>
                    }
                )
            )
            field(
                label: "Issue",
                control: "issue",
                select(
                    attrs: topcoat::view::attributes! { id="issue" name="issue" },
                    <option value="">"Toutes"</option>
                    for (value, label, chosen) in &outcomes {
                        <option value=(*value) selected=(*chosen)>(*label)</option>
                    }
                )
            )
        )
        if lines.is_empty() {
            empty_state(message: "Rien à montrer pour ces filtres.")
        } else {
            table_card(
                table(
                    table_header(table_row(
                        table_head("Quand") table_head("Opérateur") table_head("Rôle") table_head("Où")
                        table_head("Action") table_head("Sur") table_head("Issue")
                    ))
                    table_body(
                        for line in &lines {
                            table_row(
                                table_cell(<span class="tabular-nums">(line.when.clone())</span>)
                                table_cell((line.who.clone()))
                                table_cell(<span class="text-muted-foreground">(line.role)</span>)
                                table_cell((line.place.clone()))
                                table_cell(<span class="font-mono text-xs">(line.action.clone())</span>)
                                table_cell(<span class="font-mono text-xs">(line.target.clone())</span>)
                                table_cell(
                                    <span class=(if line.refused { "text-destructive" } else { "" })>(line.outcome)</span>
                                    <span class="ml-2 text-xs text-muted-foreground tabular-nums">(line.status.to_string())</span>
                                )
                            )
                        }
                    )
                )
            )
            pagination(page: page_number, page_size: PAGE_SIZE, total: total)
        }
    })
}
