#!/usr/bin/env bash
# Extract every diagnostic code literal and every error variant in the
# workspace, so the registries are built from the source rather than from a
# hand audit. A hand count of the code literals missed two entries.
#
# Sections:
#   codes    every string literal matching the code grammar, with its site
#   errors   every `#[error]` variant of every workspace error enum
#   sites    every warning or diagnostic push, counted per file
#
# Pass a section name to print only that one.
set -euo pipefail

cd "$(dirname "$0")/.."

crates="powerio powerio-dist powerio-pkg powerio-matrix powerio-prob powerio-capi powerio-cli powerio-py powerio-diag"

sources() {
  for crate in $crates; do
    [ -d "$crate/src" ] && find "$crate/src" -name '*.rs'
  done
}

codes() {
  echo "== codes: string literals matching NAMESPACE.SCOPE.SPECIFIC"
  # shellcheck disable=SC2016
  sources | xargs grep -Hno '"[A-Z][A-Z0-9_]*\(\.[A-Z0-9_]\+\)\{2,\}"' \
    | sed 's/"//g' | sort -t: -k3 | sort -u -t: -k3,3 -s
}

errors() {
  echo "== errors: #[error] variants per enum"
  for crate in $crates; do
    [ -f "$crate/src/error.rs" ] || continue
    count=$(grep -c '#\[error' "$crate/src/error.rs" || true)
    echo "-- $crate/src/error.rs ($count variants)"
    grep -n -A2 '#\[error' "$crate/src/error.rs" \
      | grep -oE '^[0-9]+[:-][[:space:]]*[A-Z][A-Za-z0-9]+[ ({,]' \
      | sed 's/[ ({,]$//' | sort -u -t: -k2
  done
}

sites() {
  echo "== sites: warning and diagnostic pushes per file"
  sources | xargs grep -Hc 'warnings\.push\|diagnostics\.push\|warnings\.extend\|diagnostics\.extend' \
    | grep -v ':0$' | sort -t: -k2 -rn
}

case "${1:-all}" in
  codes) codes ;;
  errors) errors ;;
  sites) sites ;;
  all) codes; echo; errors; echo; sites ;;
  *) echo "usage: $0 [codes|errors|sites|all]" >&2; exit 2 ;;
esac
