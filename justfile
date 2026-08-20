default: run

# Fetch bundled binaries (ffmpeg, whisper, yt-dlp, rclone, exiftool)
fetch:
    cargo run --bin fetch-resources

# Linux fast-dev codegen knobs (nightly-only). Injected at the CLI instead of
# committed to Cargo.toml/.cargo/config.toml so the checked-in build stays
# stable + cross-platform (Windows/macOS). Needs nightly + the cranelift
# component + sccache installed; harmless to drop if you only have stable.
fast := "--config 'unstable.codegen-backend=true' --config 'profile.dev.codegen-backend=\"cranelift\"' --config 'profile.dev.package.\"*\".codegen-backend=\"llvm\"'"

# Dev run — build scope capped so total system RAM stays under 90% of the
# 7.2GB budget (≈6.5GB incl. ~1-2GB host baseline). MemoryHigh=5800M clears the
# app crate's ~5GB Cranelift codegen peak without throttling; overshoot spills
# to zram swap (compressed, so 3GB swap ≈ ~1GB physical) before the 6400M kill.
run:
    systemd-run --user --scope --unit=tulipix-run \
      -p MemoryHigh=5800M -p MemoryMax=6400M -p MemorySwapMax=3000M \
      --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
      --setenv=RUSTC_WRAPPER=sccache \
      nice -n 15 ionice -c3 \
      cargo +nightly {{fast}} run -j 1 -p tulipix-app

# Dev with hot Slint reload: full app, interpreter-backed UI that re-reads
# .slint files at runtime (Slint live-preview). Separate target dir so the
# dev-reload feature set never invalidates the default build's cache.
# Memory-capped like `run` — the cold target-dev build has the same RAM peak.
#
# INSTANT-ITERATE TUNING (differs from `run`, which favours cold-cache reuse):
#  • NO sccache wrapper — sccache refuses to cache incremental crates and so
#    silently forces incremental=false. Dropping it lets `profile.dev`'s
#    incremental=true actually fire, giving function-level rebuilds of the crate
#    you just edited (sec-tools / tulipix-app) instead of full recodegen. This is
#    fingerprint-transparent (same rustc bytes) so it does NOT invalidate the
#    cached deps — crucially skia-bindings, which can only rebuild WITH network.
#  • mold stays via .cargo/config's target rustflags — we deliberately set NO
#    RUSTFLAGS env here: any RUSTFLAGS change alters every crate's fingerprint and
#    forces a full skia rebuild (offline-impossible). That ruled out -Zthreads.
# Net: edit one fn → relink in seconds. First no-sccache build recompiles only
# the workspace crates once (to seed the incremental cache); deps stay cached.
run-dev:
    systemd-run --user --scope --unit=tulipix-run-dev \
      -p MemoryHigh=5800M -p MemoryMax=6400M -p MemorySwapMax=infinity \
      --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
      --setenv=SLINT_LIVE_PREVIEW=1 \
      --setenv=CARGO_TARGET_DIR=target-dev \
      nice -n 15 ionice -c3 \
      cargo +nightly {{fast}} run -j 1 -p tulipix-app --features dev-reload

# Fast cold-build dev loop — renders with the lean femtovg backend, so NO
# skia-bindings C++ build, the single biggest compile cost. No feature trade-off
# left: every section builds either way now that playback is out-of-process mpv
# and nothing needs the GL texture path. Uses a SEPARATE target dir so it never
# invalidates the skia-cached `run-dev` artifacts (switching renderer feature
# would otherwise force a full rebuild back and forth). Same .slint hot-reload.
run-dev-lite:
    systemd-run --user --scope --unit=tulipix-run-dev-lite \
      -p MemoryHigh=5800M -p MemoryMax=6400M -p MemorySwapMax=infinity \
      --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
      --setenv=SLINT_LIVE_PREVIEW=1 \
      --setenv=CARGO_TARGET_DIR=target-dev-lite \
      nice -n 15 ionice -c3 \
      cargo +nightly {{fast}} run -j 1 -p tulipix-app \
        --no-default-features --features renderer-femtovg,alloc-mimalloc,dev-reload,ai-onnx,genesis,finances

# Sub-2s error feedback loop: `bacon` runs `cargo check` on every save (no
# codegen, no link, incremental) in the dev-reload feature set + target-dev dir,
# so it shares cache with `run-dev` and never fights it. Pair it with a live
# `run-dev` app: bacon catches type errors instantly, the app hot-reloads .slint,
# and you only restart `run-dev` when you actually change Rust behaviour.
# One-time: `cargo install bacon`. Capped so a cold check can't freeze the box.
watch:
    systemd-run --user --scope --unit=tulipix-watch \
      -p MemoryHigh=4800M -p MemoryMax=5600M -p MemorySwapMax=infinity \
      --setenv=CARGO_TARGET_DIR=target-dev \
      nice -n 18 ionice -c3 \
      bacon --job check -- -p tulipix-app --features dev-reload

# ── Rust hot-patch (no-restart section reloading) ───────────────────────────
# Two panes:
#   pane 1: `just hot`      — runs the app; routes section wire-up through the
#                             tulipix-hot dylib via hot-lib-reloader.
#   pane 2: `just hot-lib`  — watches + rebuilds ONLY that dylib on section edits.
# Edit a section callback → pane 2 rebuilds the small .so (seconds) → the running
# app re-wires it live. main.rs (9.5k lines) never recompiles during the loop.
# Limits: only section `wire()`-registered callbacks reload; editing main.rs or
# the dylib's public fn signatures still needs a `just hot` restart; section
# `static`/`OnceLock` state resets on reload (DBs on disk survive).
hot:
    systemd-run --user --scope --unit=tulipix-hot-app \
      -p MemoryHigh=5800M -p MemoryMax=6400M -p MemorySwapMax=infinity \
      --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
      --setenv=SLINT_LIVE_PREVIEW=1 \
      --setenv=CARGO_TARGET_DIR=target-dev \
      nice -n 15 ionice -c3 \
      cargo +nightly {{fast}} run -j 4 -p tulipix-app --features hot

# Watch + rebuild only the tulipix-hot dylib. Mirrors `hot`'s target dir + flags
# + feature so the rebuilt .so ABI matches the running app. One-time:
# `cargo install cargo-watch`. Light build (≤2 crates) — no memory scope needed.
hot-lib:
    cargo watch -w crates/tulipix-hot -w crates/tulipix-sec-tools -s 'just _hot-build'

# (internal) single dylib rebuild fired by `hot-lib`'s watcher.
_hot-build:
    CARGO_TARGET_DIR=target-dev \
      nice -n 18 ionice -c3 \
      cargo +nightly {{fast}} build -p tulipix-hot --features dev-reload

# Run one core crate's unit tests. Own target dir on purpose: the app's
# target-dev/target-dev-lite caches are built with the `fast` cranelift flags
# above, and a plain cargo invocation into either mixes codegen backends and
# makes the next warm app build fail. Core crates pull no renderer, so this is
# a light build and needs no memory scope.
#   just test-crate tulipix-finances
test-crate crate:
    CARGO_TARGET_DIR=target-test \
      nice -n 18 ionice -c3 \
      cargo test -j 2 -p {{crate}}

# Caps tracing
trace:
    cargo run -p tulipix-app -- --trace-caps

# CLI
cli *args:
    cargo run -p tulipix-cli -- {{args}}

# Lint + format. Capped and scoped for four reasons a bare `cargo clippy` gets
# wrong here:
#  1. Own target dir. A plain cargo invocation into target/ or target-dev* mixes
#     codegen backends with the `fast` cranelift builds above and makes the next
#     warm app build fail — the same trap `test-crate` documents.
#  2. femtovg, not the default renderer. --workspace picks up tulipix-app's
#     default features, which select renderer-skia and start the skia-bindings
#     C++ build: ~40 min and network-dependent, so effectively impossible here.
#     tulipix-app is the only workspace crate with non-empty default features, so
#     --no-default-features costs the others nothing.
#     Known gap: everything shipped (`run`, `release-*`, `installer-*`) is skia,
#     and this recipe lints the other arm of every renderer cfg in main.rs. The
#     default-features recipe had the mirror-image gap and could not be run here
#     at all; CI is femtovg in every job too, so a release build is the only
#     thing that compiles the skia arms. Small surface — keep it that way.
#  3. genesis/finances re-named. Not for their crates: tulipix-sec-genesis and
#     tulipix-sec-finances are workspace members and get linted regardless. It is
#     the six #[cfg(feature = ...)] blocks INSIDE main.rs that wire those sections
#     in — --no-default-features switches them off, and they stop being compiled
#     at all. That is how genesis shipped broken for a cycle (see the `finances`
#     comment in crates/tulipix-app/Cargo.toml).
#  4. Memory scope + -j 1, like every other build. An unbounded clippy over the
#     whole workspace is exactly the shape that froze the box three times.
lint:
    CARGO_TARGET_DIR=target-lint \
      systemd-run --user --scope --unit=tulipix-lint \
      -p MemoryHigh=4800M -p MemoryMax=5600M -p MemorySwapMax=3000M \
      nice -n 18 ionice -c3 \
      cargo clippy --workspace --all-targets -j 1 \
        --no-default-features \
        --features tulipix-app/renderer-femtovg,tulipix-app/alloc-mimalloc,tulipix-app/genesis,tulipix-app/finances \
        -- -D warnings
    cargo fmt --all --check

# WCAG AAA contrast audit on ui/tokens.slint
contrast:
    node scripts/contrast-check.mjs

fmt:
    cargo fmt --all

# Release build (current host).
#
# `full` is not implied by `default` — ort/download-binaries fetches the native
# runtime at build time, so putting it in `default` would hang a network
# dependency on every bare `cargo build` and `bacon` run. It has to be named
# here instead, and .github/workflows/release.yml names it in all three platform
# jobs. Without it the editor's Upscale silently degrades to Lanczos and
# Colorize errors out of `make_coloriser`, so a locally-built release binary is
# not the binary that ships.
release-linux:
    cargo build -p tulipix-app --release --features full --target x86_64-unknown-linux-gnu

# Size-lean release: femtovg renderer, no embedded video player (external mpv
# only). Smallest binary / fastest cold start. Use where the in-app player isn't
# needed. Deliberately NOT `full`: ORT plus its native runtime is the largest
# single thing this build exists to leave out.
#
# genesis + finances ARE named, because --no-default-features drops them and
# they are not merely crates — six #[cfg(feature = ...)] blocks inside main.rs
# wire those sections in, so without them the build has no Genesis and no
# Finances at all. That is how Genesis shipped broken for a cycle; see the
# `finances` comment in crates/tulipix-app/Cargo.toml.
release-lite:
    cargo build -p tulipix-app --release --no-default-features \
      --features renderer-femtovg,alloc-mimalloc,genesis,finances

# Absolute-minimum binary (nightly, Linux): lite build + recompiled std with
# panic_immediate_abort, dropping panic-formatting/unwinding machinery from std.
# Needs the rust-src component (`rustup component add rust-src`). Experimental —
# panic messages become abort-only; verify before shipping.
release-min:
    cargo +nightly build -p tulipix-app --release \
      --no-default-features --features renderer-femtovg,alloc-mimalloc,genesis,finances \
      -Z build-std=std,panic_abort -Z build-std-features=panic_immediate_abort \
      --target x86_64-unknown-linux-gnu

release-win:
    cargo build -p tulipix-app --release --features full --target x86_64-pc-windows-msvc

release-mac:
    cargo build -p tulipix-app --release --features full --target aarch64-apple-darwin

# Installers (host-specific tooling).
#
# Linux mirrors CI: build once with `release-linux`, then package that exact
# binary. A bare `cargo deb` would rebuild with default features and drop
# `full`, shipping an installer whose contents differ from `just release-linux`.
installer-linux: release-linux
    cargo deb -p tulipix-app --no-build --target x86_64-unknown-linux-gnu

installer-win:
    cargo wix -p tulipix-app --features full

installer-mac:
    cargo bundle --release -p tulipix-app --features full

# Desktop identity for an UNINSTALLED dev run (Linux). Wayland has no window-icon
# call: the compositor matches the toplevel app_id ("tulipix") against an installed
# desktop entry and reads Icon= from it. Running out of the build directory there is
# no such entry, so the title bar and the dock show the generic Wayland mark. This
# installs the shipped entry into the user prefix with Exec pointed at the dev
# script. Run once; re-run after changing packaging/linux/tulipix.desktop.
install-desktop:
    install -Dm644 resources/icons/tulipix-256.png \
      ~/.local/share/icons/hicolor/256x256/apps/tulipix.png
    install -Dm644 resources/icons/tulipix-64.png \
      ~/.local/share/icons/hicolor/64x64/apps/tulipix.png
    install -Dm644 resources/icons/tulipix-app.svg \
      ~/.local/share/icons/hicolor/scalable/apps/tulipix.svg
    mkdir -p ~/.local/share/applications
    sed 's|^Exec=.*|Exec={{justfile_directory()}}/run-dev-lite.sh|' \
      packaging/linux/tulipix.desktop \
      > ~/.local/share/applications/tulipix.desktop
    -gtk-update-icon-cache -f -t ~/.local/share/icons/hicolor 2>/dev/null
    -update-desktop-database ~/.local/share/applications 2>/dev/null
    @echo "installed: tulipix.desktop + hicolor icons (app_id=tulipix)"
