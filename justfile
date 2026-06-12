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
run-dev:
    systemd-run --user --scope --unit=tulipix-run-dev \
      -p MemoryHigh=5800M -p MemoryMax=6400M -p MemorySwapMax=infinity \
      --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
      --setenv=RUSTC_WRAPPER=sccache \
      --setenv=SLINT_LIVE_PREVIEW=1 \
      --setenv=CARGO_TARGET_DIR=target-dev \
      nice -n 15 ionice -c3 \
      cargo +nightly {{fast}} run -j 4 -p tulipix-app --features dev-reload

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
