//! What the processes under test spend while a run goes: each one's CPU
//! (in cores, from `/proc/<pid>/stat`) and resident memory, sampled every
//! second, and the size of the files that grow with the load (the stores
//! and their write-ahead logs).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

/// Linux's clock ticks per second for `/proc/<pid>/stat`, on every
/// architecture it runs these on.
const TICKS_PER_SEC: f64 = 100.0;

#[derive(Clone)]
pub struct Watched {
    pub name: String,
    pub pid: u32,
}

#[derive(Default, Serialize, Clone)]
pub struct Usage {
    /// Cores, on average over the window.
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub rss_max_mb: f64,
    /// Gone before the window ended.
    pub exited: bool,
}

#[derive(Default)]
struct Window {
    samples: BTreeMap<String, (f64, f64, f64, u32, bool)>,
}

pub struct Probe {
    window: Arc<Mutex<Window>>,
    files: Vec<PathBuf>,
}

impl Probe {
    pub fn start(processes: Vec<Watched>, files: Vec<PathBuf>) -> Self {
        let window = Arc::new(Mutex::new(Window::default()));
        let shared = window.clone();
        tokio::spawn(async move {
            let mut last: BTreeMap<u32, (f64, Instant)> = BTreeMap::new();
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tick.tick().await;
                for process in &processes {
                    let Some((cpu_secs, rss_mb)) = read(process.pid) else {
                        shared
                            .lock()
                            .unwrap()
                            .samples
                            .entry(process.name.clone())
                            .or_default()
                            .4 = true;
                        continue;
                    };
                    let now = Instant::now();
                    if let Some((before, at)) = last.insert(process.pid, (cpu_secs, now)) {
                        let cores = (cpu_secs - before) / now.duration_since(at).as_secs_f64();
                        let mut window = shared.lock().unwrap();
                        let entry = window.samples.entry(process.name.clone()).or_default();
                        entry.0 += cores;
                        entry.1 = entry.1.max(cores);
                        entry.2 = entry.2.max(rss_mb);
                        entry.3 += 1;
                    }
                }
            }
        });
        Self { window, files }
    }

    /// The window's usage per process, and a fresh window.
    pub fn take(&self) -> BTreeMap<String, Usage> {
        let window = std::mem::take(&mut *self.window.lock().unwrap());
        window
            .samples
            .into_iter()
            .map(|(name, (sum, max, rss, n, exited))| {
                let usage = Usage {
                    cpu_avg: if n == 0 { 0.0 } else { sum / n as f64 },
                    cpu_max: max,
                    rss_max_mb: rss,
                    exited,
                };
                (name, usage)
            })
            .collect()
    }

    /// The watched files' sizes now, in megabytes.
    pub fn file_sizes(&self) -> BTreeMap<String, f64> {
        self.files
            .iter()
            .filter_map(|path| {
                let len = std::fs::metadata(path).ok()?.len();
                Some((path.display().to_string(), len as f64 / 1_048_576.0))
            })
            .collect()
    }
}

/// CPU seconds so far, and resident megabytes.
fn read(pid: u32) -> Option<(f64, f64)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may hold spaces; the fields after it do not.
    let fields: Vec<&str> = stat.rsplit_once(')')?.1.split_whitespace().collect();
    let utime: f64 = fields.get(11)?.parse().ok()?;
    let stime: f64 = fields.get(12)?.parse().ok()?;
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let rss_kb: f64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(((utime + stime) / TICKS_PER_SEC, rss_kb / 1024.0))
}
