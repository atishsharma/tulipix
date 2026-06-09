default: run

# Fetch bundled binaries (ffmpeg, whisper, yt-dlp, rclone, exiftool)
fetch:
    cargo run --bin fetch-resources

# Linux fast-dev codegen knobs (nightly-only). Injected at the CLI instead of
# committed to Cargo.toml/.cargo/config.toml so the checked-in build stays
# stable + cross-platform (Windows/macOS). Needs nightly + the cranelift
# component + sccache installed; harmless to drop if you only have stable.
fast := '--config unstable.codegen-backend=true --config profile.dev.codegen-backend="cranelift" --config profile.dev.package."*".codegen-backend="llvm"'

# Dev run — capped at 5GB RAM, zero swap. An overshoot OOM-kills the build,
# never the host (zram swap is physical RAM, so swap stays off-limits).
# Drops debuginfo + single rustc to fit the ~10k-line app crate in 5GB.
run:
    systemd-run --user --scope --unit=tulipix-run \
      -p MemoryHigh=4600M -p MemoryMax=5000M -p MemorySwapMax=0 \
      --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
      --setenv=RUSTC_WRAPPER=sccache \
      nice -n 15 ionice -c3 \
      cargo +nightly {{fast}} run -j 1 -p tulipix-app

# Dev with hot Slint reload
run-dev:
    RUSTC_WRAPPER=sccache cargo +nightly {{fast}} run -p tulipix-app --features dev-reload

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
