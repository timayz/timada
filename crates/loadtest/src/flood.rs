//! `flood`: one form posted again and again at one address (or several in
//! turn), to cost what a
//! node does at every request whatever the answer (the rate limits' checks,
//! which ask the other nodes: `scripts/limits.sh`). No shop, no storefront
//! and no stand-in for Stripe: the address, the form, how many at once.
//!
//! The requests are sent by `--concurrency` senders, each as soon as its last
//! one was answered, or with `--rate` at that many a second whatever the
//! answers' pace, the way a flood arrives. The report counts the answers by
//! status and by where they send on, how long they took, and what each
//! watched process spent meanwhile: its processor, its memory and its open
//! files (a connection is one).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Args;
use hdrhistogram::Histogram;
use reqwest::header::LOCATION;

use crate::probe::{self, Watched};

#[derive(Args)]
pub struct Flood {
    /// Where the form is posted; given several times, each address in turn,
    /// as a balancer in front of them would.
    #[arg(long = "url", required = true)]
    urls: Vec<String>,
    /// A field of the form, as name=value (repeatable). In a value `{n}` is
    /// the request's number and `{ip}` an address made from it, so every
    /// request may name another client.
    #[arg(long = "field", value_parser = parse_pair)]
    fields: Vec<(String, String)>,
    /// A header, as name=value (repeatable), with the same `{n}` and `{ip}`.
    #[arg(long = "header", value_parser = parse_pair)]
    headers: Vec<(String, String)>,
    /// Senders at once, each waiting for its answer before the next request.
    #[arg(long, default_value_t = 32)]
    concurrency: usize,
    /// Requests per second, sent whatever the answers' pace, instead of
    /// `--concurrency` senders.
    #[arg(long)]
    rate: Option<f64>,
    #[arg(long, default_value_t = 20)]
    secs: u64,
    /// Seconds before a request is given up.
    #[arg(long, default_value_t = 30)]
    timeout_secs: u64,
    /// A process to sample, as name=pid (repeatable).
    #[arg(long = "watch", value_parser = crate::parse_watched)]
    watch: Vec<Watched>,
}

fn parse_pair(value: &str) -> Result<(String, String), String> {
    let (name, value) = value.split_once('=').ok_or("expected name=value")?;
    Ok((name.to_owned(), value.to_owned()))
}

/// `{n}` and `{ip}` filled in for request `n`.
fn fill(template: &str, n: u64) -> String {
    let ip = format!("10.{}.{}.{}", (n >> 16) & 255, (n >> 8) & 255, n & 255);
    template.replace("{n}", &n.to_string()).replace("{ip}", &ip)
}

/// What the answers were and how long they took.
struct Answers {
    latency: Histogram<u64>,
    /// By status and, for a redirect, where to.
    kinds: BTreeMap<String, u64>,
}

/// What one watched process was seen to hold at most.
#[derive(Default, Clone, Copy)]
struct Peak {
    rss_mb: f64,
    open_files: usize,
}

impl Flood {
    pub async fn run(self) -> Result<()> {
        let client = reqwest::Client::builder()
            .user_agent("timada-loadtest")
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(self.timeout_secs))
            .pool_max_idle_per_host(4096)
            .build()?;
        let answers = Arc::new(Mutex::new(Answers {
            // A microsecond to two minutes, three significant digits.
            latency: Histogram::new_with_bounds(1, 120_000_000, 3).expect("valid bounds"),
            kinds: BTreeMap::new(),
        }));
        let flood = Arc::new(self);
        let sent = Arc::new(AtomicU64::new(0));

        let peaks = Arc::new(Mutex::new(vec![Peak::default(); flood.watch.len()]));
        let sampler = tokio::spawn({
            let (flood, peaks) = (flood.clone(), peaks.clone());
            async move {
                loop {
                    for (watched, peak) in flood.watch.iter().zip(peaks.lock().unwrap().iter_mut())
                    {
                        if let Some((_, rss_mb)) = probe::read(watched.pid) {
                            peak.rss_mb = peak.rss_mb.max(rss_mb);
                        }
                        if let Ok(open) = std::fs::read_dir(format!("/proc/{}/fd", watched.pid)) {
                            peak.open_files = peak.open_files.max(open.count());
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        });
        let cpu = |flood: &Flood| -> Vec<f64> {
            flood
                .watch
                .iter()
                .map(|watched| probe::read(watched.pid).map_or(0.0, |(cpu_secs, _)| cpu_secs))
                .collect()
        };
        let cpu_before = cpu(&flood);
        let started = Instant::now();
        let until = started + Duration::from_secs(flood.secs);

        let mut tasks = tokio::task::JoinSet::new();
        match flood.rate {
            None => {
                for _ in 0..flood.concurrency {
                    let (flood, client, answers, sent) =
                        (flood.clone(), client.clone(), answers.clone(), sent.clone());
                    tasks.spawn(async move {
                        while Instant::now() < until {
                            let n = sent.fetch_add(1, Ordering::Relaxed);
                            flood.post(&client, n, &answers).await;
                        }
                    });
                }
            }
            Some(rate) => {
                let every = Duration::from_secs_f64(1.0 / rate.max(0.001));
                let mut next = started;
                while next < until {
                    tokio::time::sleep_until(next.into()).await;
                    next += every;
                    let n = sent.fetch_add(1, Ordering::Relaxed);
                    let (flood, client, answers) = (flood.clone(), client.clone(), answers.clone());
                    tasks.spawn(async move { flood.post(&client, n, &answers).await });
                }
            }
        }
        // What the flood itself lasted: the answers still owed are waited
        // for, and counted, but the processor is read over the whole of it.
        let sending = started.elapsed();
        while tasks.join_next().await.is_some() {}
        let elapsed = started.elapsed();
        let cpu_after = cpu(&flood);
        sampler.abort();

        let answers = answers.lock().unwrap();
        let answered = answers.latency.len();
        let ms = |quantile: f64| answers.latency.value_at_quantile(quantile) as f64 / 1000.0;
        println!(
            "{answered} requests in {:.1} s ({:.1} s sending): {:.0} a second; p50 {:.1} ms, p99 {:.1} ms, longest {:.1} ms",
            elapsed.as_secs_f64(),
            sending.as_secs_f64(),
            answered as f64 / elapsed.as_secs_f64(),
            ms(0.5),
            ms(0.99),
            answers.latency.max() as f64 / 1000.0,
        );
        for (kind, count) in &answers.kinds {
            println!("  {count} × {kind}");
        }
        let peaks = peaks.lock().unwrap();
        for (index, watched) in flood.watch.iter().enumerate() {
            let used = cpu_after[index] - cpu_before[index];
            println!(
                "  {}: {:.2} cores, {:.3} ms of processor per request, {:.0} MiB, {} open files at most",
                watched.name,
                used / elapsed.as_secs_f64(),
                used * 1000.0 / (answered.max(1)) as f64,
                peaks[index].rss_mb,
                peaks[index].open_files,
            );
        }
        Ok(())
    }

    async fn post(&self, client: &reqwest::Client, n: u64, answers: &Mutex<Answers>) {
        let form: Vec<(&str, String)> = self
            .fields
            .iter()
            .map(|(name, value)| (name.as_str(), fill(value, n)))
            .collect();
        let mut request = client
            .post(&self.urls[n as usize % self.urls.len()])
            .form(&form);
        for (name, value) in &self.headers {
            request = request.header(name, fill(value, n));
        }
        let at = Instant::now();
        let kind = match request.send().await {
            Ok(response) => {
                let status = response.status();
                let to = response
                    .headers()
                    .get(LOCATION)
                    .and_then(|to| to.to_str().ok())
                    .map(str::to_owned);
                // The body is read, as a browser would.
                let _ = response.bytes().await;
                match to {
                    Some(to) => format!("{} to {to}", status.as_u16()),
                    None => status.as_u16().to_string(),
                }
            }
            Err(err) if err.is_timeout() => "no answer in time".to_owned(),
            Err(err) => format!("{:#}", anyhow::Error::from(err).context("not sent")),
        };
        let took = at.elapsed().as_micros().clamp(1, u64::MAX as u128) as u64;
        let mut answers = answers.lock().unwrap();
        answers.latency.saturating_record(took);
        *answers.kinds.entry(kind).or_default() += 1;
    }
}

/// Reads a flood's settings early, so a mistake in them shows before any
/// request is sent.
pub fn check(flood: &Flood) -> Result<()> {
    for url in &flood.urls {
        reqwest::Url::parse(url).with_context(|| format!("{url:?} is not an address"))?;
    }
    anyhow::ensure!(
        flood.rate.is_some() || flood.concurrency > 0,
        "--concurrency must be above 0"
    );
    Ok(())
}
