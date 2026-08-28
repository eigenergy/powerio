#!/usr/bin/env bash
# The exported pio_* entry point set may shrink only on a head that raises
# PIO_ABI_VERSION above its stack parent's (the newest first-parent merge's
# second parent, else the fork point with main). The set is the header's
# declared entry points, extracted the way capi-header-parity.sh extracts
# them, never every token in the file. When a PowerIO.jl checkout sits
# beside the repo (or POWERIO_JL points at one), the head's exports must
# cover every :pio_* symbol that binding names; the checkout is required
# unless POWERIO_JL_OPTIONAL=1 states the run has none.
set -euo pipefail
cd "$(dirname "$0")/.."

exports_at() {
  git show "$1:powerio-capi/include/powerio.h" 2>/dev/null \
    | grep -oE 'pio_[a-z0-9_]+ *\(' \
    | grep -oE 'pio_[a-z0-9_]+' | sort -u
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
if [ -z "$head_version" ] || [ -z "$parent_version" ]; then
  echo "error: could not read PIO_ABI_VERSION at HEAD or at the stack parent $parent" >&2
  exit 1
fi
lost="$(comm -23 <(exports_at "$parent") <(exports_at HEAD))"
if [ -n "$lost" ] && [ "$head_version" -le "$parent_version" ]; then
  echo "error: exported pio_* entry points removed without a PIO_ABI_VERSION increase" >&2
  echo "(head v$head_version, parent v$parent_version):" >&2
  echo "$lost" >&2
  exit 1
fi
echo "export monotonicity holds (head v$head_version, parent v$parent_version)"

jl="${POWERIO_JL:-../PowerIO.jl}"
if [ -d "$jl/src" ]; then
  named="$(grep -rhoE ':pio_[a-z0-9_]+' "$jl/src" \
    | grep -oE 'pio_[a-z0-9_]+' | sort -u)"
  missing="$(comm -23 <(printf '%s\n' "$named") <(exports_at HEAD))"
  if [ -n "$missing" ]; then
    echo "error: the PowerIO.jl checkout names symbols this head does not export:" >&2
    echo "$missing" >&2
    exit 1
  fi
  echo "companion symbol coverage holds ($(printf '%s\n' "$named" | grep -c .) named)"
elif [ "${POWERIO_JL_OPTIONAL:-0}" = 1 ]; then
  echo "companion symbol coverage skipped: POWERIO_JL_OPTIONAL=1 and no checkout at $jl"
else
  echo "error: no PowerIO.jl checkout at $jl; set POWERIO_JL or POWERIO_JL_OPTIONAL=1" >&2
  exit 1
fi
