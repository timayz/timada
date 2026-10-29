//! The crowd: visits arrive at random (a Poisson process) at the step's
//! rate, whatever the server's pace, the way shoppers do; each runs on its
//! own. A ramp raises the rate step by step until the shop gets slow or
//! fails, and the last step it held is its capacity.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::journey::{self, Context};
use crate::metrics::{Summary, Window};
use crate::probe::{Probe, Usage};

/// One rate held for a while.
#[derive(Clone, Copy, Serialize)]
pub struct Step {
    /// New visits per second.
    pub rate: f64,
    pub secs: u64,
}

/// When a step counts as more than the shop can take.
#[derive(Clone, Copy, Serialize)]
pub struct Limits {
    pub p99_ms: f64,
    pub error_pct: f64,
    pub order_p99_ms: f64,
}

#[derive(Serialize)]
pub struct StepReport {
    pub rate: f64,
    pub secs: f64,
    /// Visits under way when the step ended.
    pub active: usize,
    pub summary: Summary,
    pub processes: BTreeMap<String, Usage>,
    pub files_mb: BTreeMap<String, f64>,
    /// Why the step was more than the shop can take; empty when it held.
    pub breaches: Vec<String>,
}

#[derive(Serialize)]
pub struct Outcome {
    pub steps: Vec<StepReport>,
    /// The highest rate a step held, if any did.
    pub capacity: Option<f64>,
    /// The visits still under way after the last step, let finish.
    pub drain: Summary,
    pub total: Summary,
}

pub struct Runner {
    pub context: Arc<Context>,
    pub probe: Probe,
    pub seed: u64,
    pub max_visits: usize,
    pub drain: Duration,
    /// Stop at the first step that breaches the limits.
    pub stop_on_breach: bool,
    pub limits: Limits,
}

impl Runner {
    pub async fn run(self, steps: &[Step]) -> Outcome {
        let metrics = self.context.http.metrics();
        let slots = Arc::new(Semaphore::new(self.max_visits));
        let mut rng = StdRng::seed_from_u64(self.seed);
        let mut reports = Vec::new();
        let mut total = Window::default();
        metrics.take();
        self.probe.take();

        for step in steps {
            let started = Instant::now();
            let end = started + Duration::from_secs(step.secs);
            let mut next = started;
            while next < end {
                tokio::time::sleep_until(next.into()).await;
                // The seed of every visit is drawn whether it runs or not, so
                // a replay with more room keeps the others identical.
                let seed = rng.random::<u64>();
                match slots.clone().try_acquire_owned() {
                    Ok(slot) => {
                        let context = self.context.clone();
                        tokio::spawn(async move {
                            journey::visit(&context, seed).await;
                            drop(slot);
                        });
                    }
                    // The generator is full: the shop is too slow for this
                    // rate, and the visit is lost as a shopper would leave.
                    Err(_) => metrics.count("visit.dropped"),
                }
                let gap = -(1.0 - rng.random::<f64>()).ln() / step.rate;
                next += Duration::from_secs_f64(gap);
            }
            let window = metrics.take();
            let secs = started.elapsed().as_secs_f64();
            let report = StepReport {
                rate: step.rate,
                secs,
                active: self.max_visits - slots.available_permits(),
                breaches: self.breaches(&window, secs),
                summary: window.summary(secs),
                processes: self.probe.take(),
                files_mb: self.probe.file_sizes(),
            };
            crate::report::step_line(&report);
            total.merge(&window);
            let breached = !report.breaches.is_empty();
            reports.push(report);
            if breached && self.stop_on_breach {
                break;
            }
        }

        let draining = Instant::now();
        while slots.available_permits() < self.max_visits && draining.elapsed() < self.drain {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        let left = self.max_visits - slots.available_permits();
        if left > 0 {
            metrics.anomaly(
                "visits_stuck",
                format!(
                    "{left} visits still under way {:?} after the last step",
                    self.drain
                ),
            );
        }
        let drain = metrics.take();
        total.merge(&drain);
        let all_secs: f64 =
            reports.iter().map(|r| r.secs).sum::<f64>() + draining.elapsed().as_secs_f64();
        let capacity = reports
            .iter()
            .take_while(|r| r.breaches.is_empty())
            .map(|r| r.rate)
            .last();
        Outcome {
            capacity,
            drain: drain.summary(draining.elapsed().as_secs_f64()),
            total: total.summary(all_secs),
            steps: reports,
        }
    }

    fn breaches(&self, window: &Window, secs: f64) -> Vec<String> {
        let summary = window.summary(secs);
        let mut breaches = Vec::new();
        if summary.latency.p99 > self.limits.p99_ms {
            breaches.push(format!(
                "p99 {:.0} ms > {:.0} ms",
                summary.latency.p99, self.limits.p99_ms
            ));
        }
        if summary.error_pct > self.limits.error_pct {
            breaches.push(format!(
                "errors {:.2} % > {:.2} %",
                summary.error_pct, self.limits.error_pct
            ));
        }
        if summary.order_latency.p99 > self.limits.order_p99_ms {
            breaches.push(format!(
                "order p99 {:.0} ms > {:.0} ms",
                summary.order_latency.p99, self.limits.order_p99_ms
            ));
        }
        // The dashboard did not answer a checkout within the storefront's wait.
        let unanswered = window.event("checkout.refused.payments_unavailable");
        if unanswered > 0 {
            breaches.push(format!("{unanswered} checkouts unanswered"));
        }
        let dropped = window.event("visit.dropped");
        if dropped > 0 {
            breaches.push(format!("{dropped} visits dropped: the generator is full"));
        }
        breaches
    }
}
