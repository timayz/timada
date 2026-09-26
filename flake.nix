{
  description = "A very basic flake";

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
            cargo-tarpaulin
            cargo-edit
            tailwindcss_4
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
            # timada-admin's build.rs runs Tailwind; use the packaged CLI instead of downloading it.
            export TAILWIND_CLI="$(command -v tailwindcss)"

            # Playwright browsers come from the Nix store, not `npx playwright install`.
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
            export PLAYWRIGHT_BROWSERS_PATH="${playwright-browsers}"

            # topcoat-cli (`topcoat dev`, `topcoat asset bundle`, `topcoat ui`) is not in
            # nixpkgs; install it with cargo, pinned to the topcoat version the workspace uses.
            export PATH="$HOME/.cargo/bin:$PATH"
            TOPCOAT_CLI_VERSION="0.9.0"
            if ! cargo install --list 2>/dev/null | grep -q "^topcoat-cli v$TOPCOAT_CLI_VERSION:"; then
              cargo install topcoat-cli --version "$TOPCOAT_CLI_VERSION" --locked
            fi
          '';
        };
      }
    );
}
