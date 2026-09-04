#!/usr/bin/env bash
# List every item deprecated for removal, so the 0.11.0 release can assert the
# set is empty. Run with --assert-empty to fail when any remain.
set -euo pipefail

cd "$(dirname "$0")/.."

rust=$(grep -rn '#\[deprecated' --include='*.rs' . \
        | grep -v '^\./target/' \
        | sort || true)
python=$(grep -rn '^_RENAMED_IN_' --include='*.py' python/ | sort || true)

count=0
if [ -n "$rust" ]; then
  echo "Rust items deprecated for removal:"
  echo "$rust"
  count=$((count + $(echo "$rust" | wc -l | tr -d ' ')))
fi
if [ -n "$python" ]; then
  echo "Python names deprecated for removal:"
  echo "$python"
  count=$((count + $(echo "$python" | wc -l | tr -d ' ')))
fi
echo "total: $count"

if [ "${1:-}" = "--assert-empty" ] && [ "$count" -ne 0 ]; then
  echo "error: 0.11.0 carries no deprecated items; remove the ones listed above" >&2
  exit 1
fi
