use anyhow::{Result, ensure};
use clap::Args;

use crate::cluster::{mark_for_rejoin, mark_for_rejoin_without_journal};
use crate::config::Config;

#[derive(Args, Debug)]
pub struct RejoinArgs {
    /// Drop this node's consensus journal with its store: for a node left
    /// out when the cluster began again from one node's copy (`snapshot
    /// restore --from-one-node`)
    #[arg(long)]
    pub drop_journal: bool,
}

/// Marks the store for a copy from a peer at the next `serve`: for a node the
/// others gave up on (the log says so and the cluster page shows it), or
/// whose store file was lost. Run with the server stopped. With the mark in
/// place `serve` empties the store, keeps its consensus journal, recreates
/// the read models and opens the node by joining, which installs a peer's
/// store page by page before anything is served. A store file that is gone
/// is simply created empty first: the node then comes back under its own id
/// through evento's amnesia fence.
///
/// With `--drop-journal` the journal goes too: the node was left out when
/// the others began a new cluster from one node's copy, and the journal it
/// kept is the old cluster's, under which the new one hands it nothing.
pub async fn rejoin(config: Config, args: RejoinArgs) -> Result<()> {
    ensure!(
        config.cluster.is_some(),
        "a single node has no peer to copy the store from: nothing to rejoin"
    );
    ensure!(
        !crate::cluster::founding_mark(&config.database.path).exists(),
        "the store is marked as what a new cluster begins with (`snapshot restore \
         --from-one-node`): start the node, or restore it again"
    );
    let marker = if args.drop_journal {
        mark_for_rejoin_without_journal(&config.database.path)?
    } else {
        mark_for_rejoin(&config.database.path)?
    };
    tracing::info!(
        marker = %marker.display(),
        journal_dropped = args.drop_journal,
        "the next start copies the store from a peer"
    );
    Ok(())
}
