{
  description = "Timada's development shell: the Rust toolchain, Playwright, the Stripe CLI, Tailwind and mkcert";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url  = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, nixpkgs-unstable, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        pkgs-unstable = import nixpkgs-unstable {
          inherit system;
        };

        # Playwright comes from nixpkgs-unstable rather than a third-party flake.
        # Note nixpkgs-unstable must stay recent enough: webkit is in the closure
        # of playwright-test either way, and older revs fail to build it with
        # `auto-patchelf could not satisfy dependency <lib>` (libhyphen, then
        # libmanette, cf. pietdevries94/playwright-web-flake#24). If that error
        # reappears, `nix flake update nixpkgs-unstable`.
        #
        # `selectBrowsers` rather than `browsers-chromium`: the latter also drops
        # chromium-headless-shell, which Playwright >= 1.49 launches for headless
        # chromium. This keeps chromium, chromium-headless-shell and ffmpeg.
        playwright-browsers = pkgs-unstable.playwright-driver.selectBrowsers {
          withFirefox = false;
          withWebkit = false;
        };
      in
      {
        devShells.default = with pkgs; mkShell {
          buildInputs = [
            gcc
            lsof
            glib
            openssl
            pkg-config
            cargo-watch
            cargo-machete
            stripe-cli
            tailwindcss_4
            nodejs_24 # `npx prettier` on the Playwright specs
            pkgs-unstable.playwright-test
            (writeShellScriptBin "mcp-server-playwright" ''
              export PWMCP_PROFILES_DIR_FOR_TEST="$PWD/.pwmcp-profiles"
              exec ${pkgs-unstable.playwright-mcp}/bin/playwright-mcp "$@"
            '')
            mkcert
            sqlite
            (rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rust-analyzer" ];
            })
          ];
          shellHook = ''
            # Link with the LLD that already ships inside the Rust toolchain, so no
            # extra package is needed. A `cargo test --workspace` performs ~23 link
            # steps and GNU ld dominates them. Deliberately not in
            # .cargo/config.toml: CI (.github/workflows/check.yml) has no lld on
            # PATH and would fail to link. An existing RUSTFLAGS wins.
            if [ "$(uname -s)" = "Linux" ] && [ -z "''${RUSTFLAGS:-}" ]; then
              _lld_dir="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/gcc-ld"
              if [ -x "$_lld_dir/ld.lld" ]; then
                export RUSTFLAGS="-C link-arg=-B$_lld_dir -C link-arg=-fuse-ld=lld"
              fi
              unset _lld_dir
            fi

            # timada-dashboard's build.rs runs Tailwind; use the packaged CLI instead of downloading it.
            export TAILWIND_CLI="$(command -v tailwindcss)"

            # Playwright browsers come from the Nix store, not `npx playwright install`.
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
            export PLAYWRIGHT_BROWSERS_PATH="${playwright-browsers}"

            # topcoat-cli (`topcoat dev`, `topcoat asset bundle`, `topcoat ui`) is not in
            # nixpkgs; install it with cargo, pinned to the topcoat version the workspace uses.
            export PATH="$HOME/.cargo/bin:$PATH"
            TOPCOAT_CLI_VERSION="0.11.0"
            if ! cargo install --list 2>/dev/null | grep -q "^topcoat-cli v$TOPCOAT_CLI_VERSION:"; then
              cargo install topcoat-cli --version "$TOPCOAT_CLI_VERSION" --locked
            fi
          '';
        };
      }
    );
}
