use anyhow::{Result, bail};
use clap::Args;

use crate::cluster::mark_for_catch_up;
use crate::config::Config;
use crate::db;

#[derive(Args, Debug)]
pub struct RebuildArgs {
    /// The read models to rebuild, by name (`--list` names them)
    #[arg(required_unless_present = "list")]
    pub read_models: Vec<String>,
    /// Name the read models instead
    #[arg(long)]
    pub list: bool,
}

/// Empties the named read models and forgets the cursors of the
/// subscriptions that fill them, with the server stopped: after a change to
/// a read model's tables or handlers, the next `serve` goes through the
/// store from the start for them and, as after a copy of the store, lets
/// every read model catch up before it listens (the `catch-up` mark next to
/// the store). On a cluster, run it on one node at a time: a node that is
/// stopped is out of the balancer, and the others serve meanwhile.
pub async fn rebuild(config: Config, args: RebuildArgs) -> Result<()> {
    if args.list {
        for read_model in db::read_models() {
            // The command's answer, for the shell.
            println!("{}", read_model.name.unwrap_or_default());
        }
        return Ok(());
    }
    let mut chosen = Vec::new();
    for name in &args.read_models {
        match db::read_model(name) {
            Some(read_model) => chosen.push(read_model),
            None => bail!("no read model is called {name}; `rebuild --list` names them"),
        }
    }
    super::refuse_if_serving(&config).await?;
    let store = db::open_store(&config.database.path).await?;
    for read_model in chosen {
        db::rebuild_read_model(&store, read_model).await?;
        tracing::info!(read_model = read_model.name, "emptied, cursor forgotten");
    }
    let mark = mark_for_catch_up(&config.database.path)?;
    tracing::info!(mark = %mark.display(), "the next start fills the read models again before serving");
    Ok(())
}
