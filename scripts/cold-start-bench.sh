#!/usr/bin/env bash
# Cold-start benchmark — measures icon-click → first-interactive frame.
# Spawns the release binary with TULIPIX_COLD_START_BENCH=1, which makes the
# app emit a single `cold_start_ms` tracing event on its first paint and exit.
# CI gate: fails if any per-OS median exceeds the target budget.
#
# Usage:  scripts/cold-start-bench.sh [N]    (default N=5 runs)

set -euo pipefail

N="${1:-5}"
BIN="${TULIPIX_BIN:-target/release/tulipix}"

case "$(uname -s)" in
  Linux*)  BUDGET_MS=500 ;;
  Darwin*) BUDGET_MS=450 ;;
  MINGW*|MSYS*|CYGWIN*) BUDGET_MS=500 ;;
  *)       BUDGET_MS=500 ;;
esac

if [[ ! -x "$BIN" ]]; then
  echo "cold-start-bench: $BIN not found — run \`cargo build --release -p tulipix-app\` first" >&2
  exit 2
fi

samples=()
for i in $(seq 1 "$N"); do
  out="$(TULIPIX_COLD_START_BENCH=1 "$BIN" 2>&1 || true)"
  ms="$(printf '%s\n' "$out" | grep -oE 'cold_start_ms=[0-9]+' | head -1 | cut -d= -f2)"
  if [[ -z "$ms" ]]; then
    echo "run $i: no cold_start_ms emitted" >&2
    exit 3
  fi
  echo "run $i: ${ms} ms"
  samples+=("$ms")
done

# Sort and pick the median.
IFS=$'\n' sorted=($(printf '%s\n' "${samples[@]}" | sort -n))
unset IFS
median="${sorted[$((${#sorted[@]} / 2))]}"

echo "median: ${median} ms (budget ${BUDGET_MS} ms)"
if (( median > BUDGET_MS )); then
  echo "cold-start-bench: FAIL — median ${median} ms > budget ${BUDGET_MS} ms" >&2
  exit 1
fi
echo "cold-start-bench: PASS"
