//! What a run tells the terminal: a line per step as it ends, then the
//! slowest routes, the journey's counters and the anomalies. The whole of it
//! goes to a JSON file too, for comparing runs.

use crate::load::{Outcome, StepReport};

pub fn step_line(step: &StepReport) {
    let s = &step.summary;
    let cpu: f64 = step.processes.values().map(|u| u.cpu_avg).sum();
    let rss: f64 = step.processes.values().map(|u| u.rss_max_mb).sum();
    println!(
        "{:>7.1}/s  {:>7.1} rps  p50 {:>6.1}  p99 {:>7.1} ms  err {:>5.2} %  orders {:>4} (p99 {:>6.0} ms)  \
         active {:>5}  cpu {:>4.1}  rss {:>6.0} MB  {}",
        step.rate,
        s.rps,
        s.latency.p50,
        s.latency.p99,
        s.error_pct,
        s.events.get("order.shown").copied().unwrap_or(0),
        s.order_latency.p99,
        step.active,
        cpu,
        rss,
        if step.breaches.is_empty() {
            "ok".to_owned()
        } else {
            step.breaches.join("; ")
        }
    );
}

pub fn print(outcome: &Outcome) {
    let total = &outcome.total;
    println!();
    match outcome.capacity {
        Some(rate) => println!("held up to {rate:.1} new visits/s"),
        None => println!("no step held within the limits"),
    }
    println!(
        "{} requests, {:.2} % failed, p50 {:.1} ms, p99 {:.1} ms; paid to order shown p50 {:.0} ms, p99 {:.0} ms",
        total.requests,
        total.error_pct,
        total.latency.p50,
        total.latency.p99,
        total.order_latency.p50,
        total.order_latency.p99,
    );

    println!("\nslowest routes (p99):");
    let mut routes: Vec<_> = total.routes.iter().collect();
    routes.sort_by(|a, b| b.1.latency.p99.total_cmp(&a.1.latency.p99));
    for (name, route) in routes.iter().take(12) {
        let failures = route
            .failures
            .iter()
            .map(|(why, n)| format!("{why}×{n}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "  {name:<40} {:>7}  p50 {:>7.1}  p99 {:>8.1}  max {:>8.1} ms  {failures}",
            route.requests, route.latency.p50, route.latency.p99, route.latency.max,
        );
    }

    println!("\njourney:");
    for (event, n) in &total.events {
        println!("  {event:<44} {n:>8}");
    }

    if total.anomalies.is_empty() {
        println!("\nno anomalies");
    } else {
        println!("\nANOMALIES:");
        for (kind, n) in &total.anomalies {
            println!("  {kind:<44} {n:>8}");
        }
        for sample in &total.samples {
            println!("    {sample}");
        }
    }
}
