mod migrate;
mod rejoin;
mod seed;
mod serve;
mod snapshot;

pub use migrate::*;
pub use rejoin::*;
pub use seed::*;
pub use serve::*;
pub use snapshot::*;

use anyhow::{Context, Result, bail};

use crate::config::Config;

/// Two processes on one node's files would each run their own consensus
/// over the same journal, and a copy taken while the server writes would be
/// torn: `seed` and `snapshot` run only while the server is stopped, which
/// the server's port tells.
pub(crate) async fn refuse_if_serving(config: &Config) -> Result<()> {
    let host = config.server.host.as_str();
    let port = config.server.port;
    match tokio::net::TcpListener::bind((host, port)).await {
        Ok(probe) => drop(probe),
        Err(err) if err.kind() == std::io::ErrorKind::AddrInUse => {
            bail!("{host}:{port} is in use: stop the server (`timada-dashboard serve`) first")
        }
        Err(err) => return Err(err).with_context(|| format!("cannot probe {host}:{port}")),
    }
    Ok(())
}
