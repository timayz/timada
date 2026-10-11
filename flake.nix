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
          config = {
            # androidenv is unfree and needs the SDK license accepted here;
            # /etc/nixos settings do not apply to a flake's own nixpkgs import.
            allowUnfree = true;
            android_sdk.accept_license = true;
          };
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

        # Android SDK for the Sparkling (Lynx) native mobile app in ./mobile
        # (Android shell in ./mobile/android).
        # Platform 37 is what compileSdk 37 (androidx.core 1.19) needs; the
        # emulator image stays on API 34 (targetSdk). build-tools 37.0.0 provides
        # the aapt2 AGP 9 is pointed at (see GRADLE_OPTS below). No NDK/CMake:
        # Lynx and the Sparkling methods ship prebuilt AARs.
        androidComposition = pkgs.androidenv.composeAndroidPackages {
          platformVersions = [ "34" "37.0" ];
          buildToolsVersions = [ "37.0.0" ];
          platformToolsVersion = "latest";
          includeEmulator = true;
          includeSystemImages = true;
          systemImageTypes = [ "google_apis" ];
          abiVersions = [ "x86_64" ];
          includeNDK = false;
          includeCmake = false;
          includeSources = false;
        };
        androidSdk = androidComposition.androidsdk;
        androidHome = "${androidSdk}/libexec/android-sdk";
        androidAapt2BuildTools = "37.0.0";
        avdName = "timada-api34";
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
            imagemagick # mobile/scripts/icons.mjs
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

            # Sparkling / Lynx toolchain. Node ^22 || ^24 is required by
            # sparkling-app-cli; AGP 9 / Gradle 9 need JDK 17. Gradle itself comes
            # from the template's wrapper (android/gradlew), not nixpkgs.
            nodejs_24
            jdk17
            androidSdk
            (writeShellScriptBin "android-avd-create" ''
              exec avdmanager create avd -n "${avdName}" \
                -k "system-images;android-34;google_apis;x86_64" -d pixel_6 --force "$@"
            '')
            (writeShellScriptBin "android-emulator" ''
              # `-gpu host` is required here: with the default (auto) and
              # swiftshader_indirect backends the emulator segfaults a few
              # seconds after the Lynx app starts rendering. Flags passed on
              # the command line come after and override these defaults.
              exec emulator -avd "${avdName}" -gpu host -no-boot-anim "$@"
            '')
          ];
          shellHook = ''
            # Link with the LLD that already ships inside the Rust toolchain, so no
            # extra package is needed. A `cargo test --workspace` performs ~23 link
            # steps and GNU ld dominates them. Deliberately not in
            # .cargo/config.toml: CI runs timayz/.github's rust-test.yml, which has
            # no lld on PATH and would fail to link. An existing RUSTFLAGS wins.
            if [ "$(uname -s)" = "Linux" ] && [ -z "''${RUSTFLAGS:-}" ]; then
              _lld_dir="$(rustc --print sysroot)/lib/rustlib/$(rustc -vV | sed -n 's/^host: //p')/bin/gcc-ld"
              if [ -x "$_lld_dir/ld.lld" ]; then
                export RUSTFLAGS="-C link-arg=-B$_lld_dir -C link-arg=-fuse-ld=lld"
              fi
              unset _lld_dir
            fi

            # timada-admin's build.rs runs Tailwind; use the packaged CLI instead of downloading it.
            export TAILWIND_CLI="$(command -v tailwindcss)"

            # Playwright browsers come from the Nix store, not `npx playwright install`.
            export PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1
            export PLAYWRIGHT_BROWSERS_PATH="${playwright-browsers}"

            # topcoat-cli (`topcoat dev`, `topcoat asset bundle`, `topcoat ui`) is not in
            # nixpkgs; install it with cargo, pinned to the topcoat version the workspace uses.
            export PATH="$HOME/.cargo/bin:$PATH"
            TOPCOAT_CLI_VERSION="0.10.0"
            if ! cargo install --list 2>/dev/null | grep -q "^topcoat-cli v$TOPCOAT_CLI_VERSION:"; then
              cargo install topcoat-cli --version "$TOPCOAT_CLI_VERSION" --locked
            fi

            # Android SDK for `sparkling doctor` / `sparkling run:android` / android/gradlew.
            export ANDROID_HOME="${androidHome}"
            export ANDROID_SDK_ROOT="$ANDROID_HOME"
            export JAVA_HOME="${pkgs.jdk17.home}"
            # The Nix store is read-only and Maven's prebuilt aapt2 is not patched
            # for NixOS, so point AGP at the SDK's build-tools copy instead.
            export GRADLE_OPTS="-Dorg.gradle.project.android.aapt2FromMavenOverride=$ANDROID_HOME/build-tools/${androidAapt2BuildTools}/aapt2"
          '';
        };
      }
    );
}
