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
use crate::cluster::{Cluster, NodeHealth, PeerState};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::store_api::StoreConnections;
use crate::ui::{
    Shell, Tone, document, empty_row, fact, facts, icons, page_header, section_card, status_badge,
};
use crate::workers::{WorkerState, Workers};
use topcoat::icon::icon;

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
