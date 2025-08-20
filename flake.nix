{
  description = "A very basic flake";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-24.11";
    # nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url  = "github:numtide/flake-utils";
    playwright.url = "github:pietdevries94/playwright-web-flake";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, playwright, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let 
        playwright-overlay = final: prev: {
          inherit (playwright.packages.${system}) playwright-test playwright-driver;
        };
        overlays = [ (import rust-overlay) playwright-overlay ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };
      in
      {
        devShells.default = with pkgs; mkShell {
          buildInputs = [
            gcc
            glib
            openssl
            pkg-config
            cargo-watch
            tailwindcss
            playwright-test
            mkcert
            (rust-bin.stable.latest.default.override {
              extensions = [ "rust-src" "rust-analyzer" ];
            })
          ];
          shellHook = ''
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
            export PLAYWRIGHT_BROWSERS_PATH="${pkgs.playwright-driver.browsers}"
            export DOCKER_SOCK="$XDG_RUNTIME_DIR/docker.sock"
          '';
        };
      }
    );
}
