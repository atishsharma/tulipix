#!/usr/bin/env bash
# Double-clickable launcher for the LITE dev build (femtovg, hot-reload).
# The dev binary can't be launched directly from a file manager — it needs the
# repo as its working dir plus SLINT_LIVE_PREVIEW + the dev-reload watcher env.
# This script sets all of that, capped at a safe memory budget (no infinite swap).
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")"

exec systemd-run --user --scope --unit=tulipix-run-dev-lite \
  -p MemoryHigh=5800M -p MemoryMax=6400M -p MemorySwapMax=3000M \
  --setenv=CARGO_PROFILE_DEV_DEBUG=0 \
  --setenv=SLINT_LIVE_PREVIEW=1 \
  --setenv=CARGO_TARGET_DIR=target-dev-lite \
  nice -n 15 ionice -c3 \
  cargo +nightly \
    --config 'unstable.codegen-backend=true' \
    --config 'profile.dev.codegen-backend="cranelift"' \
    --config 'profile.dev.package."*".codegen-backend="llvm"' \
    run -j 1 -p tulipix-app \
    --no-default-features \
    --features renderer-femtovg,alloc-mimalloc,dev-reload,lazy-whisper,lazy-ai-ep,ai-onnx
