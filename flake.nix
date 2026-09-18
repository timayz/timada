{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-26.05";
    nixpkgs-unstable.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url  = "github:numtide/flake-utils";
    # playwright.url = "github:pietdevries94/playwright-web-flake";
  };

  outputs = { self, nixpkgs, nixpkgs-unstable, rust-overlay, flake-utils, /*playwright,*/ ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        # playwright-overlay = final: prev: {
        #   inherit (playwright.packages.${system}) playwright-test playwright-driver;
        # };
        overlays = [ (import rust-overlay) /*playwright-overlay*/ ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
        pkgs-unstable = import nixpkgs-unstable {
          inherit system;
        };
      in
      {
        devShells.default = with pkgs; mkShell {
          # packages = [
          #   (writeShellScriptBin "mcp-server-playwright" ''
          #     export PWMCP_PROFILES_DIR_FOR_TEST="$PWD/.pwmcp-profiles"
          #     exec ${pkgs-unstable.playwright-mcp}/bin/mcp-server-playwright "$@"
          #   '')
          # ];
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
            # playwright-test
            mkcert
            sqlite
            (rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rust-analyzer" ];
            })
          ];
          shellHook = ''
            # timada-admin's build.rs runs Tailwind; use the packaged CLI instead of downloading it.
            export TAILWIND_CLI="$(command -v tailwindcss)"

            # topcoat-cli (`topcoat dev`, `topcoat asset bundle`, `topcoat ui`) is not in
            # nixpkgs; install it with cargo, pinned to the topcoat version the workspace uses.
            export PATH="$HOME/.cargo/bin:$PATH"
            TOPCOAT_CLI_VERSION="0.8.1"
            if ! cargo install --list 2>/dev/null | grep -q "^topcoat-cli v$TOPCOAT_CLI_VERSION:"; then
              cargo install topcoat-cli --version "$TOPCOAT_CLI_VERSION" --locked
            fi
          '';
          # shellHook = ''
          #   export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
          #   export PLAYWRIGHT_BROWSERS_PATH="${pkgs.playwright-driver.browsers}"
          # '';
        };
      }
    );
}
