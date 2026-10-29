//! What a run measured: a latency histogram and the failures per route, the
//! journey's counters (sessions, carts, payments, orders, refusals) and the
//! anomalies worth a look. Collected in windows, one per ramp step, and
//! merged into the run's total.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use hdrhistogram::Histogram;
use serde::Serialize;

/// Anomalies kept word for word per window; past it they are only counted.
const ANOMALY_SAMPLES: usize = 50;

#[derive(Default)]
pub struct Metrics {
    window: Mutex<Window>,
}

impl Metrics {
    pub fn request(&self, route: &str, took: Duration, failure: Option<String>) {
        let mut window = self.window.lock().unwrap();
        let route = window.routes.entry(route.to_owned()).or_default();
        route.latency.saturating_record(micros(took));
        match failure {
            None => route.ok += 1,
            Some(failure) => *route.failures.entry(failure).or_default() += 1,
        }
    }

    pub fn count(&self, event: &str) {
        *self
            .window
            .lock()
            .unwrap()
            .events
            .entry(event.to_owned())
            .or_default() += 1;
    }

    /// From paying to the order page showing the order: the webhook, the
    /// write across the cluster, the subscriptions and the storefront's copy.
    pub fn order_latency(&self, took: Duration) {
        self.window
            .lock()
            .unwrap()
            .order_latency
            .saturating_record(micros(took));
    }

    /// Something that should not happen: counted under `kind`, and kept
    /// with its detail while there is room.
    pub fn anomaly(&self, kind: &str, detail: String) {
        tracing::warn!(kind, %detail, "anomaly");
        let mut window = self.window.lock().unwrap();
        *window.anomalies.entry(kind.to_owned()).or_default() += 1;
        if window.samples.len() < ANOMALY_SAMPLES {
            window.samples.push(format!("{kind}: {detail}"));
        }
    }

    /// The window so far, and a fresh one in its place.
    pub fn take(&self) -> Window {
        std::mem::take(&mut *self.window.lock().unwrap())
    }
}

fn micros(took: Duration) -> u64 {
    took.as_micros().clamp(1, u64::MAX as u128) as u64
}

fn histogram() -> Histogram<u64> {
    // A microsecond to two minutes, three significant digits.
    Histogram::new_with_bounds(1, 120_000_000, 3).expect("valid bounds")
}

pub struct Route {
    pub latency: Histogram<u64>,
    pub ok: u64,
    pub failures: BTreeMap<String, u64>,
}

impl Default for Route {
    fn default() -> Self {
        Self {
            latency: histogram(),
            ok: 0,
            failures: BTreeMap::new(),
        }
    }
}

impl Route {
    pub fn failed(&self) -> u64 {
        self.failures.values().sum()
    }
}

pub struct Window {
    pub routes: BTreeMap<String, Route>,
    pub events: BTreeMap<String, u64>,
    pub order_latency: Histogram<u64>,
    pub anomalies: BTreeMap<String, u64>,
    pub samples: Vec<String>,
}

impl Default for Window {
    fn default() -> Self {
        Self {
            routes: BTreeMap::new(),
            events: BTreeMap::new(),
            order_latency: histogram(),
            anomalies: BTreeMap::new(),
            samples: Vec::new(),
        }
    }
}

impl Window {
    pub fn merge(&mut self, other: &Window) {
        for (name, route) in &other.routes {
            let mine = self.routes.entry(name.clone()).or_default();
            mine.latency.add(&route.latency).expect("same bounds");
            mine.ok += route.ok;
            for (failure, n) in &route.failures {
                *mine.failures.entry(failure.clone()).or_default() += n;
            }
        }
        for (event, n) in &other.events {
            *self.events.entry(event.clone()).or_default() += n;
        }
        self.order_latency
            .add(&other.order_latency)
            .expect("same bounds");
        for (kind, n) in &other.anomalies {
            *self.anomalies.entry(kind.clone()).or_default() += n;
        }
        for sample in &other.samples {
            if self.samples.len() < ANOMALY_SAMPLES {
                self.samples.push(sample.clone());
            }
        }
    }

    pub fn requests(&self) -> u64 {
        self.routes.values().map(|r| r.ok + r.failed()).sum()
    }

    pub fn failed(&self) -> u64 {
        self.routes.values().map(Route::failed).sum()
    }

    pub fn event(&self, name: &str) -> u64 {
        self.events.get(name).copied().unwrap_or(0)
    }

    /// Every request's latency together.
    pub fn all_latency(&self) -> Histogram<u64> {
        self.latency_of(|_| true)
    }

    /// The latency of the routes `pick` names, together.
    fn latency_of(&self, pick: impl Fn(&str) -> bool) -> Histogram<u64> {
        let mut all = histogram();
        for (name, route) in &self.routes {
            if pick(name) {
                all.add(&route.latency).expect("same bounds");
            }
        }
        all
    }

    pub fn summary(&self, seconds: f64) -> Summary {
        let all = self.all_latency();
        let requests = self.requests();
        Summary {
            requests,
            rps: requests as f64 / seconds.max(0.001),
            error_pct: pct(self.failed(), requests),
            latency: Percentiles::of(&all),
            // What a shopper waits for: a page, or the answer to a form
            // (which writes, through the cluster).
            pages: Percentiles::of(&self.latency_of(|name| name.starts_with("GET /"))),
            actions: Percentiles::of(&self.latency_of(|name| name.starts_with("POST /"))),
            order_latency: Percentiles::of(&self.order_latency),
            routes: self
                .routes
                .iter()
                .map(|(name, route)| {
                    (
                        name.clone(),
                        RouteSummary {
                            requests: route.ok + route.failed(),
                            failures: route.failures.clone(),
                            latency: Percentiles::of(&route.latency),
                        },
                    )
                })
                .collect(),
            events: self.events.clone(),
            anomalies: self.anomalies.clone(),
            samples: self.samples.clone(),
        }
    }
}

fn pct(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// Milliseconds.
#[derive(Serialize, Clone, Copy, Default)]
pub struct Percentiles {
    pub count: u64,
    pub p50: f64,
    pub p90: f64,
    pub p99: f64,
    pub max: f64,
}

impl Percentiles {
    fn of(histogram: &Histogram<u64>) -> Self {
        if histogram.is_empty() {
            return Self::default();
        }
        let ms = |micros: u64| micros as f64 / 1000.0;
        Self {
            count: histogram.len(),
            p50: ms(histogram.value_at_quantile(0.50)),
            p90: ms(histogram.value_at_quantile(0.90)),
            p99: ms(histogram.value_at_quantile(0.99)),
            max: ms(histogram.max()),
        }
    }
}

#[derive(Serialize)]
pub struct RouteSummary {
    pub requests: u64,
    pub failures: BTreeMap<String, u64>,
    pub latency: Percentiles,
}

#[derive(Serialize)]
pub struct Summary {
    pub requests: u64,
    pub rps: f64,
    pub error_pct: f64,
    pub latency: Percentiles,
    pub pages: Percentiles,
    pub actions: Percentiles,
    pub order_latency: Percentiles,
    pub routes: BTreeMap<String, RouteSummary>,
    pub events: BTreeMap<String, u64>,
    pub anomalies: BTreeMap<String, u64>,
    pub samples: Vec<String>,
}
