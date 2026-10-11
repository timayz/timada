//! Compiles the Tailwind stylesheet into `$OUT_DIR/tailwind.css`, served as a
//! topcoat asset via `topcoat::tailwind::stylesheet!()`.
//!
//! `BuildConfig::render` emits no `cargo:rerun-if-*` directive of its own, and
//! any single directive replaces Cargo's default "rerun on any package file".
//! The two `rerun-if-changed` lines keep the stylesheet in sync when a utility
//! class is added to a `.rs` file; without them the class silently does not
//! exist at runtime.
fn main() {
    println!("cargo:rerun-if-changed=styles.css");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-env-changed=TAILWIND_CLI");
    topcoat::tailwind::BuildConfig::new()
        .input("styles.css")
        // The Nix dev shell exports the packaged CLI; unset, topcoat downloads
        // its pinned release.
        .executable_env("TAILWIND_CLI")
        .render()
        .unwrap();
}
