use std::path::Path;
use std::process::Command;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    let out_dir = std::env::var("OUT_DIR").unwrap_or_default();
    let input = Path::new(&manifest_dir).join("tailwind.css");
    let output = Path::new(&out_dir).join("app.css");

    println!("cargo:rerun-if-changed=tailwind.css");
    println!("cargo:rerun-if-changed=assets");
    // Re-run when any workspace crate's templates change — Tailwind scans them
    // for class names. Only existing dirs are registered (a missing path would
    // force a re-run on every build).
    let workspace_root = Path::new(&manifest_dir).join("../..");
    for group in ["crates", "apps"] {
        let Ok(entries) = std::fs::read_dir(workspace_root.join(group)) else {
            continue;
        };
        for entry in entries.flatten() {
            let templates = entry.path().join("templates");
            if templates.is_dir() {
                println!("cargo:rerun-if-changed={}", templates.display());
            }
        }
    }

    let status = Command::new("tailwindcss")
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--minify")
        .current_dir(&manifest_dir)
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(status) => panic!("tailwindcss failed with {status}"),
        Err(source) => panic!(
            "could not run the `tailwindcss` CLI ({source}). It is provided by the \
             Nix devshell — run builds inside `nix develop`."
        ),
    }
}
