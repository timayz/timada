//! Fails the build when a persisted shape changed, or when `events.lock` is
//! missing shapes. See `docs/event-evolution.md`.

use timada_event_lock::{Lock, check, scan, workspace_root};

#[test]
fn persisted_shapes_only_grow() -> anyhow::Result<()> {
    let root = workspace_root()?;
    let locked = Lock::parse(&std::fs::read_to_string(root.join("events.lock"))?)?;
    let current = scan(&root)?;
    let problems: Vec<String> = check(&locked, &current)
        .into_iter()
        .map(|problem| problem.to_string())
        .collect();
    assert!(
        problems.is_empty(),
        "\n\n{}\n\nSee docs/event-evolution.md.\n",
        problems.join("\n")
    );
    Ok(())
}
