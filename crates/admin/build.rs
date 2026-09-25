fn main() {
    // A `cargo:rerun-if-*` directive replaces Cargo's default, which is to
    // rerun whenever any file in the package changes. So naming the CLI's
    // environment variable is not free: without the two paths below, editing
    // a Tailwind class in a `.rs` file would leave the generated stylesheet
    // untouched, and the class would simply not exist at runtime.
    println!("cargo:rerun-if-env-changed=TAILWIND_CLI");
    println!("cargo:rerun-if-changed=styles.css");
    println!("cargo:rerun-if-changed=src");

    // Tailwind scans this crate's sources (see `styles.css`). The Nix devshell
    // exports `TAILWIND_CLI`; without it topcoat downloads the standalone CLI.
    let mut config = topcoat::tailwind::BuildConfig::new().input("styles.css");
    if let Ok(cli) = std::env::var("TAILWIND_CLI") {
        config = config.executable(cli);
    }
    if let Err(err) = config.render() {
        panic!("tailwind build failed: {err}");
    }
}
