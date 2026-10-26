//! `/admin/cluster`: this node, as its consensus engine sees it. Every node
//! shows its own view; open the page on each node of a cluster.

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
        format!("node {} of {}", health.node_id, health.nodes)
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
                        "This node's journal is an older copy of itself, so it refuses writes. Stop it, delete the store file, run `timada-dashboard rejoin` and start it again."
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
                    if health.joined_at_start {
                        fact(term: "Last start", "copied the store from another node")
                    }
                )
            )

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
                                    attrs: attributes! { data-node=(peer.id) data-given-up=(if peer.given_up { "true" } else { "false" }) },
                                    table_cell("Node " (peer.id))
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
