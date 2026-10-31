//! Stages the Lucide icon set the pages draw from, then compiles the Tailwind
//! stylesheet into `$OUT_DIR/tailwind.css`, served as a topcoat asset via
//! `topcoat::tailwind::stylesheet!()`.
//!
//! Neither step emits a `cargo:rerun-if-*` directive of its own, and any
//! single directive replaces Cargo's default "rerun on any package file".
//! The `rerun-if-changed` lines keep the stylesheet in sync when a utility
//! class is added to a `.rs` file; without them the class silently does not
//! exist at runtime.
fn main() {
    println!("cargo:rerun-if-changed=styles.css");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=icons");
    println!("cargo:rerun-if-env-changed=TAILWIND_CLI");
    // `iconify_icon!("lucide:…")` is checked at compile time against this
    // set. The set is kept in `icons/` next to the manifest (and committed),
    // so a build never reaches for the network and every clone draws the
    // same icons; it is fetched there once when missing.
    topcoat::icon::iconify::BuildConfig::new()
        .cache_dir("icons")
        .icon_set("lucide")
        .stage()
        .unwrap();
    let mut tailwind = topcoat::tailwind::BuildConfig::new().input("styles.css");
    // The Nix dev shell exports the packaged CLI. Unset (CI, a plain cargo
    // build), topcoat downloads its pinned release into the target directory;
    // naming the variable while it is unset would be an error.
    if std::env::var_os("TAILWIND_CLI").is_some() {
        tailwind = tailwind.executable_env("TAILWIND_CLI");
    }
    tailwind.render().unwrap();
}
