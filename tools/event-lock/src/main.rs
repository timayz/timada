//! `cargo run -p timada-event-lock -- update` rewrites `events.lock` after
//! new events, value types or views were added. It refuses to record a change
//! to a shape that is already locked; `--allow-changes` overrides that, for
//! shapes no deployed database has ever stored.

use std::process::ExitCode;

use timada_event_lock::{Lock, check, scan, workspace_root};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let allow_changes = args.iter().any(|a| a == "--allow-changes");
    match args.first().map(String::as_str) {
        Some("update") => match update(allow_changes) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("{err:#}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: timada-event-lock update [--allow-changes]");
            ExitCode::FAILURE
        }
    }
}

fn update(allow_changes: bool) -> anyhow::Result<()> {
    let root = workspace_root()?;
    let current = scan(&root)?;
    let path = root.join("events.lock");
    if path.exists() && !allow_changes {
        let locked = Lock::parse(&std::fs::read_to_string(&path)?)?;
        let broken: Vec<String> = check(&locked, &current)
            .into_iter()
            .filter(|problem| problem.is_breaking())
            .map(|problem| problem.to_string())
            .collect();
        if !broken.is_empty() {
            anyhow::bail!(
                "refusing to update events.lock:\n\n{}\n\nSee docs/event-evolution.md.",
                broken.join("\n")
            );
        }
    }
    std::fs::write(&path, current.render())?;
    println!("events.lock: {} shapes", current.len());
    Ok(())
}
