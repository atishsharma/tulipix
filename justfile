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

# Fast cold-build dev loop — drops embedded-mpv (so NO skia-bindings C++ build,
# the single biggest compile cost) and renders with the lean femtovg backend.
# Trade-off: the in-app Videos player goes inert; music/podcast/radio/YouTube
# still play via out-of-process mpv. Uses a SEPARATE target dir so it never
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

# Lint + format
lint:
    cargo clippy --workspace --all-targets -- -D warnings
    cargo fmt --all --check

# WCAG AAA contrast audit on ui/tokens.slint
contrast:
    node scripts/contrast-check.mjs

fmt:
    cargo fmt --all

# Release build (current host)
release-linux:
    cargo build -p tulipix-app --release --target x86_64-unknown-linux-gnu

# Size-lean release: femtovg renderer, no embedded video player (external mpv
# only). Smallest binary / fastest cold start. Use where the in-app player isn't
# needed.
release-lite:
    cargo build -p tulipix-app --release --no-default-features --features renderer-femtovg

# Absolute-minimum binary (nightly, Linux): lite build + recompiled std with
# panic_immediate_abort, dropping panic-formatting/unwinding machinery from std.
# Needs the rust-src component (`rustup component add rust-src`). Experimental —
# panic messages become abort-only; verify before shipping.
release-min:
    cargo +nightly build -p tulipix-app --release \
      --no-default-features --features renderer-femtovg \
      -Z build-std=std,panic_abort -Z build-std-features=panic_immediate_abort \
      --target x86_64-unknown-linux-gnu

release-win:
    cargo build -p tulipix-app --release --target x86_64-pc-windows-msvc

release-mac:
    cargo build -p tulipix-app --release --target aarch64-apple-darwin

# Installers (host-specific tooling)
installer-linux:
    cargo deb -p tulipix-app

installer-win:
    cargo wix -p tulipix-app

installer-mac:
    cargo bundle --release -p tulipix-app
