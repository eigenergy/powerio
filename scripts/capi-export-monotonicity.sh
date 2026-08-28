#!/usr/bin/env bash
# The exported pio_* set may shrink only on a head that raises
# PIO_ABI_VERSION. The stack parent is the newest first-parent merge's second
# parent (how the stack cascades), else the fork point with main. When a
# PowerIO.jl checkout sits beside the repo (or POWERIO_JL points at one), the
# head's exports must also cover every pio_* symbol that binding resolves.
set -euo pipefail
cd "$(dirname "$0")/.."

exports_at() {
  git show "$1:powerio-capi/include/powerio.h" 2>/dev/null \
    | grep -oE '\bpio_[a-z0-9_]+' | sort -u
}
version_at() {
  git show "$1:powerio-capi/src/lib.rs" 2>/dev/null \
    | grep -oE 'pub const PIO_ABI_VERSION: u32 = [0-9]+' | grep -oE '[0-9]+$'
}

parent=""
while read -r commit; do
  if [ "$(git rev-list --no-walk --count --merges "$commit")" = 1 ]; then
    parent="$(git rev-parse "$commit^2")"
    break
  fi
done < <(git rev-list --first-parent -n 200 HEAD)
if [ -z "$parent" ]; then
  parent="$(git merge-base HEAD origin/main 2>/dev/null || git merge-base HEAD main)"
fi

head_version="$(version_at HEAD)"
parent_version="$(version_at "$parent")"
if [ -n "$parent_version" ] && [ "$head_version" = "$parent_version" ]; then
  lost="$(comm -23 <(exports_at "$parent") <(exports_at HEAD))"
  if [ -n "$lost" ]; then
    echo "error: exported pio_* symbols removed without a PIO_ABI_VERSION bump:" >&2
    echo "$lost" >&2
    exit 1
  fi
fi
echo "export monotonicity holds (head v$head_version, parent v${parent_version:-?})"

jl="${POWERIO_JL:-../PowerIO.jl}"
if [ -d "$jl/src" ]; then
  resolved="$(grep -rhoE '_library_symbol\([^,]+, :pio_[a-z0-9_]+' "$jl/src" \
    | grep -oE 'pio_[a-z0-9_]+' | sort -u)"
  missing="$(comm -23 <(printf '%s\n' "$resolved") <(exports_at HEAD))"
  if [ -n "$missing" ]; then
    echo "error: the PowerIO.jl checkout resolves symbols this head does not export:" >&2
    echo "$missing" >&2
    exit 1
  fi
  echo "companion symbol coverage holds ($(printf '%s\n' "$resolved" | grep -c .) resolved)"
else
  echo "companion symbol coverage skipped: no PowerIO.jl checkout at $jl"
fi
