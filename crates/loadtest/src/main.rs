//! `timada-loadtest`: crowds of random shoppers on a storefront, to find
//! what breaks and how much the platform takes before it slows down.
//!
//! Each visit browses, searches, fills a cart and sometimes pays, over plain
//! HTTP (no browser), its choices drawn from a seed so a run replays. Payments
//! go through a stand-in for Stripe that this process serves, in front of
//! stripe-mock, which then tells the dashboard through the shop's webhook.
//!
//! `soak` holds one rate for a while (leaks, backlogs, drift); `ramp` raises
//! the rate step by step until a limit breaks, and reports the last rate held.
//! `scripts/loadtest.sh` sets up the three-node cluster, the shop and the
//! storefront, then runs this.

mod http;
mod journey;
mod load;
mod metrics;
mod probe;
mod report;
mod site;
mod stripe;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

use crate::journey::Mix;
use crate::load::{Limits, Runner, Step};
use crate::metrics::Metrics;
use crate::probe::{Probe, Watched};

#[derive(Parser)]
#[command(about = "Random shoppers on a storefront, for bugs and capacity")]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// One rate for a while.
    Soak {
        #[command(flatten)]
        common: Common,
        /// New visits per second.
        #[arg(long, default_value_t = 2.0)]
        rate: f64,
        #[arg(long, default_value_t = 600)]
        secs: u64,
        /// Seconds per report line.
        #[arg(long, default_value_t = 30)]
        every: u64,
    },
    /// Rates rising step by step until a limit breaks.
    Ramp {
        #[command(flatten)]
        common: Common,
        #[arg(long, default_value_t = 1.0)]
        start: f64,
        /// Each step's rate is the last one's times this.
        #[arg(long, default_value_t = 1.5)]
        factor: f64,
        #[arg(long, default_value_t = 500.0)]
        max: f64,
        /// Seconds per step.
        #[arg(long, default_value_t = 60)]
        step_secs: u64,
        /// Go on past a breached step (up to `max`).
        #[arg(long)]
        keep_going: bool,
    },
}

#[derive(Args)]
struct Common {
    /// The storefront.
    #[arg(
        long,
        env = "LOAD_TARGET",
        default_value = "https://chatounette.localhost"
    )]
    target: String,
    /// The dashboard, where Stripe's webhooks go.
    #[arg(
        long,
        env = "LOAD_DASHBOARD",
        default_value = "https://timada.localhost"
    )]
    dashboard: String,
    /// The webhook signing secret the shop was given (`seed --test-payments`).
    #[arg(long, default_value = "whsec_load")]
    webhook_secret: String,
    /// Where the stand-in for Stripe listens; the nodes' `payments.stripe_api_url`.
    #[arg(long, default_value = "127.0.0.1:12112")]
    stripe_listen: String,
    /// stripe-mock, behind it.
    #[arg(long, default_value = "http://localhost:12111")]
    stripe_mock: String,
    /// Milliseconds each Stripe call takes, on average.
    #[arg(long, default_value_t = 150)]
    stripe_latency_ms: u64,
    /// Accept any certificate (a local CA the system does not know).
    #[arg(long)]
    insecure: bool,
    /// Seconds before a request is given up.
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
    /// The seed every visit's choices come from.
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// Visits under way at once, past which arrivals are dropped.
    #[arg(long, default_value_t = 5000)]
    max_visits: usize,
    /// Seconds the last visits get to finish after the last step.
    #[arg(long, default_value_t = 90)]
    drain_secs: u64,

    /// Pages read after landing, on average.
    #[arg(long, default_value_t = 4.0)]
    pages: f64,
    /// Seconds between pages, on average; 0 for no pause (most pressure).
    #[arg(long, default_value_t = 3.0)]
    think_secs: f64,
    /// That a product page read ends in the cart.
    #[arg(long, default_value_t = 0.25)]
    add_to_cart: f64,
    /// That a visit with a cart checks out.
    #[arg(long, default_value_t = 0.4)]
    checkout: f64,
    /// That a shopper at the checkout pays.
    #[arg(long, default_value_t = 0.85)]
    pay: f64,

    /// A step breaks above this p99 over every request, in milliseconds.
    #[arg(long, default_value_t = 1000.0)]
    p99_ms: f64,
    /// ... or above this share of failed requests, in percent.
    #[arg(long, default_value_t = 1.0)]
    error_pct: f64,
    /// ... or above this p99 from paying to the order showing.
    #[arg(long, default_value_t = 15000.0)]
    order_p99_ms: f64,

    /// A process to sample, as name=pid (repeatable).
    #[arg(long = "watch", value_parser = parse_watched)]
    watch: Vec<Watched>,
    /// A file whose size to follow (repeatable).
    #[arg(long = "watch-file")]
    watch_files: Vec<PathBuf>,
    /// Where the full report goes, as JSON.
    #[arg(long, default_value = "data/load/report.json")]
    report: PathBuf,
}

fn parse_watched(value: &str) -> Result<Watched, String> {
    let (name, pid) = value.split_once('=').ok_or("expected name=pid")?;
    Ok(Watched {
        name: name.to_owned(),
        pid: pid.parse().map_err(|_| format!("{pid:?} is not a pid"))?,
    })
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cli = Cli::parse();
    let stop_on_breach = matches!(
        cli.mode,
        Mode::Ramp {
            keep_going: false,
            ..
        }
    );
    let (common, steps) = match cli.mode {
        Mode::Soak {
            common,
            rate,
            secs,
            every,
        } => {
            let every = every.clamp(1, secs.max(1));
            let mut steps = vec![Step { rate, secs: every }; (secs / every) as usize];
            if secs % every > 0 {
                steps.push(Step {
                    rate,
                    secs: secs % every,
                });
            }
            (common, steps)
        }
        Mode::Ramp {
            common,
            start,
            factor,
            max,
            step_secs,
            ..
        } => {
            if start <= 0.0 || factor <= 1.0 {
                bail!("--start must be above 0 and --factor above 1");
            }
            let mut steps = Vec::new();
            let mut rate = start;
            while rate <= max {
                steps.push(Step {
                    rate,
                    secs: step_secs,
                });
                rate = (rate * factor * 10.0).round() / 10.0;
            }
            (common, steps)
        }
    };
    let common = &common;

    let metrics = Arc::new(Metrics::default());
    let timeout = Duration::from_secs(common.timeout_secs);
    let http = http::Http::new(&common.target, common.insecure, timeout, metrics.clone())?;

    let stripe = stripe::FakeStripe::new(
        stripe::Settings {
            upstream: common.stripe_mock.clone(),
            latency: Duration::from_millis(common.stripe_latency_ms),
            webhook_base: common.dashboard.clone(),
            webhook_secret: common.webhook_secret.clone(),
        },
        http.client().clone(),
        metrics.clone(),
    );
    let listener = tokio::net::TcpListener::bind(&common.stripe_listen)
        .await
        .with_context(|| format!("cannot listen on {}", common.stripe_listen))?;
    stripe.serve(listener);

    let catalog = site::Catalog::discover(&mut http::Visitor::new(&http)).await?;
    eprintln!(
        "{} categories, {} products, {} search terms; {} steps",
        catalog.categories.len(),
        catalog.products.len(),
        catalog.terms.len(),
        steps.len()
    );

    let context = Arc::new(journey::Context {
        http,
        stripe,
        catalog,
        mix: Mix {
            pages: common.pages,
            think_secs: common.think_secs,
            add_to_cart: common.add_to_cart,
            checkout: common.checkout,
            pay: common.pay,
        },
    });
    let runner = Runner {
        context,
        probe: Probe::start(common.watch.clone(), common.watch_files.clone()),
        seed: common.seed,
        max_visits: common.max_visits,
        drain: Duration::from_secs(common.drain_secs),
        stop_on_breach,
        limits: Limits {
            p99_ms: common.p99_ms,
            error_pct: common.error_pct,
            order_p99_ms: common.order_p99_ms,
        },
    };
    let outcome = runner.run(&steps).await;
    report::print(&outcome);

    if let Some(dir) = common.report.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&common.report, serde_json::to_vec_pretty(&outcome)?)
        .with_context(|| format!("cannot write {}", common.report.display()))?;
    eprintln!("report: {}", common.report.display());
    if !outcome.total.anomalies.is_empty() {
        std::process::exit(2);
    }
    Ok(())
}
