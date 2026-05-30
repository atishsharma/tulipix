default: run

# Fetch bundled binaries (ffmpeg, whisper, yt-dlp, rclone, exiftool)
fetch:
    cargo run --bin fetch-resources

# Dev run
run:
    cargo run -p tulipix-app

# Dev with hot Slint reload
run-dev:
    cargo run -p tulipix-app --features dev-reload

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
