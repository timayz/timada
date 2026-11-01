use anyhow::{Result, ensure};

use crate::cluster::{Files, mark_for_rejoin};
use crate::config::Config;

/// Marks the store for a copy from a peer at the next `serve`: for a node the
/// others gave up on (the log says so and the cluster page shows it), or
/// whose store file was lost. Run with the server stopped. With the mark in
/// place `serve` empties the store, keeps its consensus journal, recreates
/// the read models and opens the node by joining, which installs a peer's
/// store page by page before anything is served. A store file that is gone
/// is simply created empty first: the node then comes back under its own id
/// through evento's amnesia fence.
pub async fn rejoin(config: Config) -> Result<()> {
    ensure!(
        config.cluster.is_some(),
        "a single node has no peer to copy the store from: nothing to rejoin"
    );
    let files = Files {
        path: config.database.path.clone(),
    };
    let marker = mark_for_rejoin(&files)?;
    tracing::info!(marker = %marker.display(), "the next start copies the store from a peer");
    Ok(())
}
