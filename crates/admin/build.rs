fn main() {
    println!("cargo:rerun-if-env-changed=TAILWIND_CLI");
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
