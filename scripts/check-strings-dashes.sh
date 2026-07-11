#!/usr/bin/env bash
# CI-runnable gate: player-facing copy in mf-game's strings table must not
# contain Unicode en dashes (U+2013) or em dashes (U+2014). Prefer ASCII
# hyphen-minus so locale files stay greppable and typography stays consistent.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${ROOT}/crates/mf-game/src/strings.rs"

if [[ ! -f "$TARGET" ]]; then
  echo "check-strings-dashes: missing $TARGET" >&2
  exit 1
fi

# Match en dash (–) or em dash (—) anywhere in the table source.
if grep -n $'[\u2013\u2014]' "$TARGET"; then
  echo "check-strings-dashes: en/em dashes found in strings.rs (use ASCII '-')" >&2
  exit 1
fi

echo "check-strings-dashes: ok (no en/em dashes in strings.rs)"
