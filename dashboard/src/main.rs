mod app;
mod auth;
mod cli;
mod cluster;
mod config;
mod db;
mod ui;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

use crate::config::Config;

/// Timada dashboard
#[derive(Parser)]
#[command(name = "timada-dashboard", about = "Timada back-office", long_about = None)]
struct Cli {
    /// Configuration file. Omitted: built-in defaults plus TIMADA__* environment variables.
    #[arg(long, global = true, env = "TIMADA_CONFIG")]
    config: Option<String>,

    /// Defaults to `serve`, so `topcoat dev` (which passes no arguments) works.
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the HTTP server
    Serve {
        /// Bind address (overrides server.host)
        #[arg(long)]
        host: Option<String>,

        /// Port (overrides server.port)
        #[arg(long)]
        port: Option<u16>,
    },
    /// Create the database if needed and apply pending migrations
    Migrate,
    /// Drop the database and recreate it from scratch (debug builds only)
    #[cfg(debug_assertions)]
    Reset,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load(cli.config.as_deref())?;

    init_tracing(&config.monitoring)?;

    match cli.command.unwrap_or(Commands::Serve {
        host: None,
        port: None,
    }) {
        Commands::Serve { host, port } => cli::serve(config, host, port).await,
        Commands::Migrate => cli::migrate(config).await,
        #[cfg(debug_assertions)]
        Commands::Reset => cli::reset(config).await,
    }
}

fn init_tracing(monitoring: &config::MonitoringConfig) -> Result<()> {
    let env_filter = EnvFilter::new(&monitoring.log_level);
    if monitoring.log_json {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .flatten_event(true)
                    .with_target(monitoring.log_target)
                    .with_line_number(monitoring.log_line_number)
                    .with_filter(env_filter),
            )
            .try_init()?;
    } else {
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_target(monitoring.log_target)
                    .with_line_number(monitoring.log_line_number)
                    .with_filter(env_filter),
            )
            .try_init()?;
    }
    Ok(())
}
