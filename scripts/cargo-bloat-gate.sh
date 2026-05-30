#!/usr/bin/env bash
# Per-crate binary-size regression gate.
#
# Builds the release binary, runs `cargo bloat --release --crates --message-format=json`,
# diffs each crate's contribution against the baseline stored in
# `.cargo-bloat-baseline.json`, and fails when any crate grows by more than
# 2% (unless the PR title or commit body carries the `[bloat-allow]` tag).
#
# Update the baseline after an intentional growth:
#   scripts/cargo-bloat-gate.sh --update-baseline

set -euo pipefail

BASELINE=".cargo-bloat-baseline.json"
LIMIT_PCT=2.0
UPDATE=0
[[ "${1:-}" == "--update-baseline" ]] && UPDATE=1

command -v cargo-bloat >/dev/null 2>&1 || {
  echo "cargo-bloat-gate: install with \`cargo install cargo-bloat\`" >&2; exit 2;
}

current="$(cargo bloat --release --crates --message-format=json 2>/dev/null \
  | jq '{crates: [.crates[] | {name: .name, size: .size}]}')"

if (( UPDATE )); then
  echo "$current" > "$BASELINE"
  echo "cargo-bloat-gate: baseline updated"
  exit 0
fi

if [[ ! -f "$BASELINE" ]]; then
  echo "cargo-bloat-gate: no baseline — writing initial $BASELINE"
  echo "$current" > "$BASELINE"
  exit 0
fi

# Honour [bloat-allow] in the PR / commit message.
if [[ -n "${GITHUB_PR_BODY:-}" && "${GITHUB_PR_BODY}" == *"[bloat-allow]"* ]]; then
  echo "cargo-bloat-gate: PR body carries [bloat-allow] — skipping enforcement"
  exit 0
fi
if git rev-parse HEAD >/dev/null 2>&1; then
  if git log -1 --pretty=%B 2>/dev/null | grep -q '\[bloat-allow\]'; then
    echo "cargo-bloat-gate: HEAD carries [bloat-allow] — skipping enforcement"
    exit 0
  fi
fi

# Diff each crate. Use jq to compute per-crate growth percent.
diff_json="$(jq -n --argjson b "$(cat "$BASELINE")" --argjson c "$current" '
  $c.crates | map({
    name: .name,
    size: .size,
    baseline: (($b.crates[] | select(.name == .name).size) // 0),
  }) | map(. + { pct: (if .baseline > 0 then ((.size - .baseline) / .baseline) * 100 else 0 end) })
')"

fail=0
echo "$diff_json" | jq -c '.[]' | while read -r row; do
  name="$(echo "$row" | jq -r .name)"
  pct="$(echo "$row" | jq -r .pct)"
  bytes="$(echo "$row" | jq -r .size)"
  if awk -v p="$pct" -v l="$LIMIT_PCT" 'BEGIN { exit (p>l)?0:1 }'; then
    printf '  REGRESSION  %-32s +%.2f%% (%d B)\n' "$name" "$pct" "$bytes" >&2
    fail=1
  fi
done

if (( fail )); then
  echo "cargo-bloat-gate: FAIL — see crates above. Add [bloat-allow] or update the baseline."
  exit 1
fi
echo "cargo-bloat-gate: PASS"
