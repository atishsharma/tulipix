#!/usr/bin/env bash
# Double-clickable launcher for the LITE dev build (femtovg, hot-reload).
# The dev binary can't be launched directly from a file manager — it needs the
# repo as its working dir plus SLINT_LIVE_PREVIEW + the dev-reload watcher env.
# This script sets all of that, capped at a safe memory budget (no infinite swap).
#
# The feature list at the bottom must stay byte-identical to `just run-dev-lite`:
# both build into target-dev-lite, so any difference re-codegens the app crate on
# every alternation between the two entry points. It had drifted to a set that
# could not build at all — `lazy-whisper` and `lazy-ai-ep` were removed from
# tulipix-app, and cargo rejects an unknown feature before compiling anything, so
# this launcher and the installed .desktop entry both failed to start. It also
# dropped genesis/finances, which --no-default-features makes mandatory to name.
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
    --features renderer-femtovg,alloc-mimalloc,dev-reload,ai-onnx,genesis,finances
