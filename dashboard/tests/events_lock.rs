//! Guards the shapes timada persists as bitcode.
//!
//! Events and the types nested in them are frozen: bitcode is positional, so
//! adding or removing a field makes every stored occurrence undecodable. A
//! snapshotted view may change only when its `.revision(n)` grows. `events.lock`
//! at the workspace root records all three; this test fails when a recorded line
//! changes.
//!
//! After adding an event, type or view:
//!
//! ```text
//! EVENTO_LOCK=update cargo test -p timada-dashboard --test events_lock
//! ```
//!
//! then commit `events.lock`.

#[test]
fn persisted_shapes_only_grow() {
    evento_lock::check(env!("CARGO_MANIFEST_DIR")).unwrap();
}
