//! `/admin/cluster`: this node, as its consensus engine sees it, and the
//! subscriptions it runs. Every node shows its own view; open the page on
//! each node of a cluster.
//!
//! The members of the cluster are changed from here too (#710): a node
//! started over an empty store is added, a node that left is removed. Any
//! member decides it with the others; none is restarted.

pub mod add;
pub mod remove;

use std::sync::Arc;

use topcoat::{
    Result,
    context::{Cx, context},
    router::{href, page, query_params},
    view::{View, attributes, view},
};

use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::cluster::{Cluster, Executor, NodeHealth, NodeId, PeerState};
use crate::components::alert::{AlertVariant, alert, alert_description};
use crate::components::button::{ButtonVariant, button};
use crate::components::field::{field, field_label};
use crate::components::input::input;
use crate::components::table::{
    table, table_body, table_cell, table_head, table_header, table_row,
};
use crate::config::Config;
use crate::mailroom::{self, OwedMail};
use crate::saga_fault;
use crate::store_api::StoreConnections;
use crate::ui::{
    Shell, Tone, document, empty_row, fact, facts, icons, notice, page_header, section_card,
    status_badge,
};
use crate::workers::{WorkerState, Workers};
use evento::DeadLetter;
use topcoat::icon::icon;

#[topcoat::router::query_params(error = bad_request)]
struct ClusterQuery {
    error: Option<String>,
    message: Option<String>,
}

/// The page, with what an action on it has to say.
pub fn cluster_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(index).query(Flash { error, message }).resolve(cx)
}

/// What a change of members still waits for, in words.
fn waits_label(waits_for: &[evento::cluster::Wait]) -> String {
    use evento::cluster::Wait;
    let waits: Vec<String> = waits_for
        .iter()
        .map(|wait| match wait {
            Wait::Drain => "the writes in flight to finish".to_owned(),
            Wait::Copy(node) => format!("node {} to copy the store", node.0),
        })
        .collect();
    if waits.is_empty() {
        "nothing more: it is about to be done".to_owned()
    } else {
        waits.join(", and ")
    }
}

/// The node an administrator most likely adds next, and where it would be
/// reached: the first number past the members, at the address this node's
/// configuration lists for it, or else this node's own address with the
/// number changed (`timada-1.timada-headless:7000` gives
/// `timada-3.timada-headless:7000`), which is how a chart names its pods.
fn next_node(members: &[NodeId], me: NodeId, peers: &[evento::cluster::PeerAddr]) -> (u64, String) {
    let next = members.iter().map(|node| node.0 + 1).max().unwrap_or(0);
    let listed = peers.get(next as usize).map(ToString::to_string);
    let guessed = || {
        let own = peers.get(me.0 as usize)?.to_string();
        let number = format!("-{}.", me.0);
        own.contains(&number)
            .then(|| own.replacen(&number, &format!("-{next}."), 1))
    };
    (next, listed.or_else(guessed).unwrap_or_default())
}

/// The most mails given up on the page lists.
const FAILED_MAILS_SHOWN: i64 = 50;

/// The most events given up on the page lists.
const GIVEN_UP_SHOWN: usize = 50;

/// The saga an event given up on belongs to, as a person reads it: its
/// subscription's key without the work group and the crate (`invoicing` for
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
    } else if !crate::cluster::in_touch(peer) {
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
    } else if health.founded_apart {
        ("Of another cluster", Tone::Warning)
    } else if health.rejoin_required || health.copy_pending {
        ("Copy needed", Tone::Warning)
    } else if !health.confirmed {
        ("Waiting for the other nodes", Tone::Neutral)
    } else if !crate::cluster::reaches_majority(health) {
        ("Out of touch with most nodes", Tone::Warning)
    } else {
        ("Serving", Tone::Success)
    }
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let query = query_params::<ClusterQuery>(cx)?;
    let health = context::<Arc<Cluster>>(cx).health();
    // The members the cluster agreed on, and the change of them in progress.
    let cluster = context::<Arc<Cluster>>(cx);
    let me = cluster.id();
    let members = cluster.members();
    let changing = members.change.as_ref().map(|change| {
        let to: Vec<String> = change.to.iter().map(|node| node.0.to_string()).collect();
        (to.join(", "), waits_label(&change.waits_for))
    });
    // Only a node of a cluster takes another: one that runs alone has no
    // `[cluster]` section to list the others in.
    let adds = context::<Config>(cx)
        .cluster
        .as_ref()
        .map(|config| next_node(&members.nodes, me, &config.peers));
    let connections = context::<Arc<StoreConnections>>(cx).by_tenant();
    let storefronts: usize = connections.values().sum();
    let (label, tone) = state(&health);
    let (kept, limit) = health.backlog;
    let nodes = if health.nodes == 1 {
        "single node".to_owned()
    } else {
        format!("node {} of {}", health.id.0, health.nodes)
    };
    // The workers not running are the ones worth a row.
    let workers = context::<Workers>(cx).states();
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
    let checkouts_queued = context::<Arc<crate::payments::Checkouts>>(cx).queued();
    // The mails the sagas owe on this node, and the ones given up on.
    let db = context::<crate::db::Db>(cx);
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
        saga_fault::dead_letters(context::<Executor>(cx), db, GIVEN_UP_SHOWN)
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
            notice(error: query.error.as_deref(), message: query.message.as_deref())

            if let Some((to, waits)) = &changing {
                alert(
                    variant: AlertVariant::Warning,
                    attrs: attributes! { role="status" class="mb-6" data-members-changing="" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "The members are changing to nodes " (to) ". It waits for " (waits) ". Writes may be refused for a few seconds meanwhile."
                    )
                )
            }

            if health.rolled_back {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "This node's journal is an older copy of itself, so it refuses writes until it copies the store and the journal back. Restart it: the copy happens at startup."
                    )
                )
            } else if health.founded_apart {
                alert(
                    variant: AlertVariant::Destructive,
                    attrs: attributes! { role="alert" class="mb-6" },
                    icon(data: icons::WARNING)
                    alert_description(
                        "The other nodes began again from a copy this node was not given, so it refuses writes. Stop it, then either put that same copy back on it (snapshot restore --from-one-node) or have it copy the store from them (rejoin --drop-journal), and start it."
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
                    if health.founded_at_start {
                        fact(term: "Last start", "began a new cluster from a copy put back")
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
                                table_head("")
                            )
                        )
                        table_body(
                            if health.peers.is_empty() {
                                empty_row(colspan: 3, text: "No other node.")
                            }
                            for peer in &health.peers {
                                table_row(
                                    attrs: attributes! { data-node=(peer.id.0) data-given-up=(if peer.given_up { "true" } else { "false" }) },
                                    table_cell("Node " (peer.id.0))
                                    table_cell((peer_label(peer)))
                                    table_cell(
                                        // Removing is for a node that left for good, or is about to:
                                        // the box is what keeps a slip of the hand from doing it.
                                        <form method="post" action=(href!(remove::remove)) class="flex flex-wrap items-center justify-end gap-3">
                                            <input type="hidden" name="node" value=(peer.id.0)>
                                            <label class="flex items-center gap-2 text-sm">
                                                <input type="checkbox" required="" name="sure" value="yes">
                                                "Node " (peer.id.0) " leaves for good"
                                            </label>
                                            button(variant: ButtonVariant::Destructive, attrs: attributes! { type="submit" data-remove-node=(peer.id.0) }, "Remove")
                                        </form>
                                    )
                                )
                            }
                        )
                    )
                )
            }

            if let Some((next, address)) = &adds {
                section_card(
                    title: Some("Add a node"),
                    description: Some("Start the new node first, over an empty store, with its number and the others' addresses in its configuration: it waits to be added here, copies the store from a member, then serves. From then on it counts: a majority is one of the members with it."),
                    <form method="post" action=(href!(add::add)) class="flex flex-wrap items-end gap-3">
                        field(
                            attrs: attributes! { class="w-28" },
                            field_label(attrs: attributes! { for="add-node" }, "Node")
                            input(attrs: attributes! { id="add-node" name="node" type="number" min="0" step="1" inputmode="numeric" required="" value=(*next) })
                        )
                        field(
                            attrs: attributes! { class="grow sm:max-w-md" },
                            field_label(attrs: attributes! { for="add-address" }, "Address the members reach it at")
                            input(attrs: attributes! { id="add-address" name="address" type="text" required="" placeholder="host:port" value=(address.as_str()) })
                        )
                        button(variant: ButtonVariant::Outline, attrs: attributes! { type="submit" }, "Add to the cluster")
                    </form>
                )
            }
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peers(addresses: &[&str]) -> Vec<evento::cluster::PeerAddr> {
        addresses
            .iter()
            .map(|address| address.parse().unwrap())
            .collect()
    }

    /// The node offered to add is the first number past the members, at
    /// the address the configuration lists for it; where the configuration
    /// stops at the members (the pods of a chart before they are restarted
    /// on the longer list), at this node's own address with the number
    /// changed; and at nothing where neither says.
    #[test]
    fn the_next_node_is_offered_at_the_address_it_would_have() {
        let members = [NodeId(0), NodeId(1), NodeId(2)];
        let listed = peers(&[
            "10.0.0.1:7000",
            "10.0.0.2:7000",
            "10.0.0.3:7000",
            "10.0.0.4:7000",
        ]);
        assert_eq!(
            next_node(&members, NodeId(1), &listed),
            (3, "10.0.0.4:7000".to_owned())
        );
        let pods = peers(&[
            "timada-0.timada-headless:7000",
            "timada-1.timada-headless:7000",
            "timada-2.timada-headless:7000",
        ]);
        assert_eq!(
            next_node(&members, NodeId(1), &pods),
            (3, "timada-3.timada-headless:7000".to_owned())
        );
        let bare = peers(&["10.0.0.1:7000", "10.0.0.2:7000", "10.0.0.3:7000"]);
        assert_eq!(next_node(&members, NodeId(0), &bare), (3, String::new()));
        // A node that left keeps its number: the next one is past the
        // highest member, not in the gap.
        assert_eq!(next_node(&[NodeId(0), NodeId(2)], NodeId(0), &bare).0, 3);
    }

    #[test]
    fn a_change_says_what_it_waits_for() {
        use evento::cluster::Wait;
        assert_eq!(
            waits_label(&[Wait::Drain, Wait::Copy(NodeId(3))]),
            "the writes in flight to finish, and node 3 to copy the store"
        );
        assert_eq!(waits_label(&[]), "nothing more: it is about to be done");
    }
}
