//! `/admin/cluster`: this node, as its consensus engine sees it, and the
//! subscriptions it runs. Every node shows its own view; open the page on
//! each node of a cluster.

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, app_context},
    router::page,
    view::{View, attributes, view},
};

use crate::app::admin::require_admin;
use crate::cluster::{Cluster, Executor, NodeHealth, PeerState};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::mailroom::{self, OwedMail};
use crate::saga_fault;
use crate::store_api::StoreConnections;
use crate::ui::{
    Shell, Tone, document, empty_row, fact, facts, icons, page_header, section_card, status_badge,
};
use crate::workers::{WorkerState, Workers};
use evento::DeadLetter;
use topcoat::icon::icon;

/// The most mails given up on the page lists.
const FAILED_MAILS_SHOWN: i64 = 50;

/// The most events given up on the page lists.
const GIVEN_UP_SHOWN: usize = 50;

/// The saga an event given up on belongs to, as a person reads it: its
/// subscription's key without the region and the crate (`invoicing` for
/// `eu.timada-dashboard/invoicing`).
fn saga_label(letter: &DeadLetter) -> &str {
    letter
        .subscription
        .rsplit_once('/')
        .map_or(letter.subscription.as_str(), |(_, name)| name)
}

/// What a mail given up on says, for its row.
fn failed_mail_label(mail: &OwedMail) -> (String, String) {
    let kind = match mail.kind() {
        Some(kind) => kind.label().to_owned(),
        None => mail.kind.clone(),
    };
    let to = if mail.tenant_id.is_empty() {
        format!("the platform's, stream {}", mail.stream_id)
    } else {
        format!("tenant {}, stream {}", mail.tenant_id, mail.stream_id)
    };
    (kind, to)
}

/// How another node stands, for the table.
fn peer_label(peer: &PeerState) -> String {
    let silent = peer.silent_for.as_secs();
    if peer.given_up {
        format!(
            "no longer waited for, silent for {}",
            format_silence(silent)
        )
    } else if silent >= 10 {
        format!("not heard from for {}", format_silence(silent))
    } else {
        "in touch".to_owned()
    }
}

fn format_silence(secs: u64) -> String {
    if secs >= 2 * 24 * 3600 {
        format!("{} days", secs / (24 * 3600))
    } else if secs >= 2 * 3600 {
        format!("{} h", secs / 3600)
    } else if secs >= 120 {
        format!("{} min", secs / 60)
    } else {
        format!("{secs} s")
    }
}

/// The word for a worker's state, and its colour; what else the row says.
fn worker_label(state: &WorkerState) -> (&'static str, Tone, String) {
    match state {
        WorkerState::Running => ("Running", Tone::Success, String::new()),
        WorkerState::Restarting {
            error,
            since,
            attempts,
        } => (
            "Restarting",
            Tone::Warning,
            format!(
                "failed {} ago, {attempts} restart{} tried: {error}",
                format_silence(crate::auth::now().saturating_sub(*since)),
                if *attempts == 1 { "" } else { "s" }
            ),
        ),
        WorkerState::Stopped { reason } => ("Stopped", Tone::Warning, reason.clone()),
    }
}

/// The word for the node's state, and its colour.
fn state(health: &NodeHealth) -> (&'static str, Tone) {
    if health.rolled_back {
        ("Journal rolled back", Tone::Warning)
    } else if health.rejoin_required || health.copy_pending {
        ("Copy needed", Tone::Warning)
    } else if !health.confirmed {
        ("Waiting for the other nodes", Tone::Neutral)
    } else {
        ("Serving", Tone::Success)
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let health = app_context::<Arc<Cluster>>(cx).health();
    let connections = app_context::<Arc<StoreConnections>>(cx).by_tenant();
    let storefronts: usize = connections.values().sum();
    let (label, tone) = state(&health);
    let (kept, limit) = health.backlog;
    let nodes = if health.nodes == 1 {
        "single node".to_owned()
    } else {
        format!("node {} of {}", health.id.0, health.nodes)
    };
    // The workers not running are the ones worth a row.
    let workers = app_context::<Workers>(cx).states();
    let running = workers
        .iter()
        .filter(|(_, state)| state.is_running())
        .count();
    let troubled: Vec<(String, &'static str, Tone, String)> = workers
        .iter()
        .filter(|(_, state)| !state.is_running())
        .map(|(name, state)| {
            let (label, tone, detail) = worker_label(state);
            (name.clone(), label, tone, detail)
        })
        .collect();
    let workers_label = if troubled.is_empty() {
        format!("all {running} running")
    } else {
        format!("{running} of {} running", workers.len())
    };
    // The requests to pay this node is answering or has waiting for a lane.
    let checkouts_queued = app_context::<Arc<crate::payments::Checkouts>>(cx).queued();
    // The mails the sagas owe on this node, and the ones given up on.
    let db = app_context::<crate::db::Db>(cx);
    let mails = mailroom::summary(db)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let failed_mails = if mails.failed > 0 {
        mailroom::failed_mails(db, FAILED_MAILS_SHOWN)
            .await
            .map_err(topcoat::Error::from_anyhow)?
    } else {
        Vec::new()
    };
    // The pictures this node has yet to fetch from the others, and the ones
    // it gave up on.
    let pictures = crate::media::fetcher::summary(db)
        .await
        .map_err(topcoat::Error::from_anyhow)?;
    let pictures_label = match (pictures.wanted, pictures.given_up) {
        (0, 0) => "all here".to_owned(),
        (wanted, 0) => format!("{wanted} to fetch from the other nodes"),
        (wanted, given_up) => {
            format!("{wanted} to fetch from the other nodes, {given_up} given up on")
        }
    };
    let mails_label = match (mails.owed, mails.failed) {
        (0, 0) => "none waiting".to_owned(),
        (owed, 0) => format!("{owed} waiting to be sent"),
        (owed, failed) => format!("{owed} waiting to be sent, {failed} given up on"),
    };
    // The events the sagas gave up on this week: a fault of the one order's
    // or cart's own, recorded on it rather than holding every tenant behind
    // it, and kept by evento as a dead letter beside the saga's cursor.
    let (given_up_count, given_up) =
        saga_fault::dead_letters(app_context::<Executor>(cx), db, GIVEN_UP_SHOWN)
            .await
            .map_err(topcoat::Error::from_anyhow)?;
    let given_up_label = match given_up_count {
        0 => "none this week".to_owned(),
        1 => "1 this week, recorded on its order or cart".to_owned(),
        n => format!("{n} this week, each recorded on its order or cart"),
    };
    let storefronts_label = match storefronts {
        0 => "none".to_owned(),
        n => format!(
            "{n}, from {} tenant{}",
            connections.len(),
            if connections.len() == 1 { "" } else { "s" }
        ),
    };

    Ok(view! {
        document(
            title: "This node",
            shell: Shell::page("admin_cluster"),
            page_header(
                title: "This node",
                subtitle: Some("The store on this node, and the other nodes as it sees them. Each node shows its own view."),
                status_badge(tone: tone, attrs: attributes! { data-node-state="" }, (label))
            )

            if health.rolled_back {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "This node's journal is an older copy of itself, so it refuses writes until it copies the store and the journal back. Restart it: the copy happens at startup."
                    )
                )
            } else if health.rejoin_required || health.copy_pending {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "The other nodes tidied up writes this node never got, so it refuses writes until it copies the store back. Restart it: the copy happens at startup."
                    )
                )
            }

            section_card(
                title: Some("Store"),
                facts(
                    fact(term: "Node", attrs: attributes! { data-node="" }, (&nodes))
                    fact(
                        term: "Writes kept",
                        attrs: attributes! { data-backlog=(kept) },
                        (kept) " of " (limit) " for nodes that are away"
                    )
                    fact(term: "Storefronts connected", attrs: attributes! { data-storefronts=(storefronts) }, (&storefronts_label))
                    fact(
                        term: "Background workers",
                        attrs: attributes! { data-workers=(workers.len()) data-workers-troubled=(troubled.len()) },
                        (&workers_label)
                    )
                    fact(
                        term: "Requests to pay",
                        attrs: attributes! { data-checkouts-queued=(checkouts_queued) },
                        (checkouts_queued) " being answered"
                    )
                    fact(
                        term: "Mails owed",
                        attrs: attributes! { data-mails-owed=(mails.owed) data-mails-failed=(mails.failed) },
                        (&mails_label)
                    )
                    fact(
                        term: "Pictures wanted",
                        attrs: attributes! { data-pictures-wanted=(pictures.wanted) data-pictures-given-up=(pictures.given_up) },
                        (&pictures_label)
                    )
                    fact(
                        term: "Events given up on",
                        attrs: attributes! { data-given-up=(given_up_count) },
                        (&given_up_label)
                    )
                    if health.joined_at_start {
                        fact(term: "Last start", "copied the store from another node")
                    }
                )
            )

            if !troubled.is_empty() {
                section_card(
                    title: Some("Workers not running"),
                    table(
                        table_header(
                            table_row(
                                table_head("Worker")
                                table_head("State")
                                table_head("Detail")
                            )
                        )
                        table_body(
                            for (name, label, tone, detail) in &troubled {
                                table_row(
                                    attrs: attributes! { data-worker=(name.as_str()) },
                                    table_cell((name))
                                    table_cell(status_badge(tone: *tone, (*label)))
                                    table_cell((detail))
                                )
                            }
                        )
                    )
                )
            }

            if !failed_mails.is_empty() {
                section_card(
                    title: Some("Mails given up on"),
                    table(
                        table_header(
                            table_row(
                                table_head("Mail")
                                table_head("To")
                                table_head("Tries")
                                table_head("Last failure")
                            )
                        )
                        table_body(
                            for (mail, (kind, to)) in failed_mails.iter().map(|mail| (mail, failed_mail_label(mail))) {
                                table_row(
                                    attrs: attributes! { data-failed-mail=(mail.stream_id.as_str()) },
                                    table_cell((kind))
                                    table_cell((to))
                                    table_cell((mail.attempts))
                                    table_cell((&mail.reason))
                                )
                            }
                        )
                    )
                )
            }

            if !given_up.is_empty() {
                section_card(
                    title: Some("Events given up on"),
                    table(
                        table_header(
                            table_row(
                                table_head("Saga")
                                table_head("Event")
                                table_head("Why")
                                table_head("When")
                            )
                        )
                        table_body(
                            for letter in &given_up {
                                table_row(
                                    attrs: attributes! { data-given-up-event=(letter.aggregate_id.as_str()) },
                                    table_cell((saga_label(letter)))
                                    table_cell((&letter.name) " of " (if letter.tenant.is_empty() { "the platform's".to_owned() } else { format!("tenant {}", letter.tenant) }) ", stream " <span class="font-mono">(&letter.aggregate_id)</span>)
                                    table_cell((saga_fault::one_line(&letter.error)))
                                    table_cell(<time datetime=(letter.timestamp)>(crate::ui::format_time(letter.timestamp))</time>)
                                )
                            }
                        )
                    )
                )
            }

            if health.nodes > 1 {
                section_card(
                    title: Some("Other nodes"),
                    table(
                        table_header(
                            table_row(
                                table_head("Node")
                                table_head("Standing")
                            )
                        )
                        table_body(
                            if health.peers.is_empty() {
                                empty_row(colspan: 2, text: "No other node.")
                            }
                            for peer in &health.peers {
                                table_row(
                                    attrs: attributes! { data-node=(peer.id.0) data-given-up=(if peer.given_up { "true" } else { "false" }) },
                                    table_cell("Node " (peer.id.0))
                                    table_cell((peer_label(peer)))
                                )
                            }
                        )
                    )
                )
            }
        )
    })
}
