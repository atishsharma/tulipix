# Tulipix — the Flutter app over the Rust bridge. `just` lists the recipes.

set shell := ["bash", "-uc"]

# 4 threads and 8 GB on this box: rustc's own parallelism plus ninja's is what
# pushes a build into swap. Cap once, here, rather than remembering per command.
export CARGO_BUILD_JOBS := env("CARGO_BUILD_JOBS", "2")
export RUSTC_WRAPPER := env("RUSTC_WRAPPER", if path_exists("/usr/bin/sccache") == "true" { "sccache" } else { "" })

root    := justfile_directory()
bridge  := root / "crates/tulipix-bridge"
app     := root / "app_flutter"
# Throwaway data dir. `seed` copies the real DBs in; nothing ever copies out.
sandbox := root / ".flutter-sandbox"

default:
    @just --justfile {{justfile()}} --list

# --- one-time setup -------------------------------------------------------

# Fill in the desktop runner scaffolding flutter_create owns (linux/, windows/,
# macos/). Safe to re-run: it only adds what is missing.
scaffold:
    cd {{app}} && flutter create --platforms=linux,windows,macos --org com.tulipix .

# Register the app with the desktop, for a run out of the build directory.
#
# Neither X11 nor Wayland carries a window icon the app can push: the shell
# matches the toplevel's app_id / WM_CLASS against an installed `.desktop` file
# and takes the icon from there. Wayland has no other route at all, so without
# this the taskbar entry is a blank square no matter what the runner does. The
# runner also loads the icon straight off disk, which covers X11 only.
desktop:
    install -Dm644 {{root}}/packaging/linux/tulipix.desktop \
        ~/.local/share/applications/tulipix.desktop
    install -Dm644 {{root}}/resources/icons/tulipix-256.png \
        ~/.local/share/icons/hicolor/256x256/apps/tulipix.png
    install -Dm644 {{root}}/resources/icons/tulipix-64.png \
        ~/.local/share/icons/hicolor/64x64/apps/tulipix.png
    -gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor 2>/dev/null
    -update-desktop-database ~/.local/share/applications 2>/dev/null
    @echo "installed: tulipix.desktop + icons (log out and in if the shell caches them)"

# Install the codegen CLI. Its version must match the `flutter_rust_bridge`
# crate version in crates/tulipix-bridge/Cargo.toml, or generate refuses.
install-codegen:
    cargo install flutter_rust_bridge_codegen --locked

# Wire cargokit into the Flutter project so `flutter build` also builds the
# bridge crate. ONE-TIME. Re-running clobbers lib/main.dart, adds a demo
# api/simple.rs that collides with api/mod.rs, and rewrites rust_builder/**
# back to expecting a library called rust_lib_tulipix -- this crate builds
# libtulipix_bridge.so, so those four build files have been corrected by hand.
integrate:
    cd {{app}} && flutter_rust_bridge_codegen integrate --rust-crate-dir ../crates/tulipix-bridge

# Generate the FFI layer: app_flutter/lib/src/rust/** and the bridge's
# frb_generated.rs. Run after every change to crates/tulipix-bridge/src/api/.
# Config lives at app_flutter/flutter_rust_bridge.yaml, written by `integrate`.
# `pub get` first: frb reads pubspec.LOCK to resolve freezed, so a pubspec edit
# that has not been resolved yet looks to it like a missing dependency.
gen:
    cd {{app}} && flutter pub get
    cd {{app}} && flutter_rust_bridge_codegen generate

# Download the bundled tools (ffmpeg, whisper, yt-dlp, rclone, exiftool) into
# resources/bin/, checked against the SHA-256 in resources/binaries.toml.
fetch:
    cd {{root}} && cargo run --bin fetch-resources

# --- dev ------------------------------------------------------------------

# NEVER point the Flutter build at the real data directory. Section schemas
# apply on open, silently and one-directionally — a Flutter build that opens
# the real photos.db can migrate it forward under the shipping app's feet.
seed:
    mkdir -p {{sandbox}}/data {{sandbox}}/config {{sandbox}}/cache
    # Self-ignoring: the sandbox holds a copy of the real settings.json and the
    # section databases.
    printf '*\n' > {{sandbox}}/.gitignore
    # Under Tulipix/, not at the root: the app reads $XDG_DATA_HOME/Tulipix and
    # $XDG_CONFIG_HOME/Tulipix, so copies left a level up are copies nothing
    # ever opens -- which is how this sandbox ran for weeks against a settings
    # file the real one had never been copied into.
    mkdir -p {{sandbox}}/data/Tulipix {{sandbox}}/config/Tulipix
    cp -n ~/.local/share/Tulipix/*.db {{sandbox}}/data/Tulipix/ 2>/dev/null || true
    cp -n ~/.config/Tulipix/settings.json {{sandbox}}/config/Tulipix/ 2>/dev/null || true
    # The transfer CA, and it has to be the real one. A phone trusts a root, not
    # a machine: let the sandbox mint its own and every paired phone meets a
    # certificate signed by a root it has never seen, has to be told to proceed
    # past the warning, and lands on an origin the browser treats as insecure --
    # no service worker, no installed PWA, and uploads that fail in ways
    # downloads do not.
    cp -rn ~/.local/share/Tulipix/transfer {{sandbox}}/data/Tulipix/ 2>/dev/null || true

dev: gen seed
    cd {{app}} && \
    XDG_DATA_HOME={{sandbox}}/data \
    XDG_CONFIG_HOME={{sandbox}}/config \
    XDG_CACHE_HOME={{sandbox}}/cache \
    flutter run -d linux --dart-define=TULIPIX_DEV=1

# The same, built for speed. `dev` is a DEBUG build, and a debug Flutter frame
# is not comparable to anything: every frame re-walks the entire render tree for
# a semantics assert, and this app keeps all ten sections in the tree at once.
# Measured on the visualizer's 11 fps that assert alone was 18% of Dart time.
# Any CPU number worth arguing about has to come from here.
#
# `profile` keeps the VM service and the timeline, so DevTools still works;
# `release` has neither and is what the thing actually feels like:
#
#     just fast release
fast mode="profile": gen seed
    cd {{app}} && \
    XDG_DATA_HOME={{sandbox}}/data \
    XDG_CONFIG_HOME={{sandbox}}/config \
    XDG_CACHE_HOME={{sandbox}}/cache \
    flutter run -d linux --{{mode}} --dart-define=TULIPIX_DEV=1

# Wipe the sandbox. The real data directory is never touched.
reseed:
    rm -rf {{sandbox}}
    @just --justfile {{justfile()}} seed

# --- checks ---------------------------------------------------------------

# Depends on `gen`: frb_generated.rs is a derived, gitignored artefact, and
# checking against a stale one reports errors that the source no longer has.
# Warm, codegen is about a second.
check: gen
    cd {{bridge}} && cargo check
    cd {{app}} && flutter analyze

test: gen
    cd {{bridge}} && cargo test
    cd {{app}} && flutter test

fmt:
    cd {{bridge}} && cargo fmt
    cd {{app}} && dart format lib test

# One library crate's tests, in its own target dir so it never fights a
# running dev build for the lock:   just test-crate tulipix-finances
test-crate crate:
    cd {{root}} && CARGO_TARGET_DIR=target-test \
      nice -n 18 ionice -c3 \
      cargo test -j 2 -p {{crate}}

# The headless CLI over tulipix-tools:   just cli convert --help
cli *args:
    cd {{root}} && cargo run -p tulipix-cli -- {{args}}
