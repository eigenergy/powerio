#!/usr/bin/env bash
# Keep the PowerIO 0.11 public vocabulary and operation names exact.
set -euo pipefail
# Resolve the repository root with shell expansion only, so the script still
# reaches the right directory when no external commands are available.
script_dir=${0%/*}
if [ "$script_dir" = "$0" ]; then
  script_dir=.
fi
cd "$script_dir/.."

# Runs one search and decides the outcome from grep's exit status. Status 0
# means the text is present, so the gate prints the label with the matching
# lines and fails. Status 1 means nothing matched. Any other status means
# grep could not run, which also fails the gate, so a missing or broken
# search tool cannot report a clean run without reading any file.
# Compiled Python caches repeat the text of their sources and are skipped.
scan() {
  local label=$1
  shift
  local hits status=0
  hits=$(grep -rnHIE --exclude-dir=__pycache__ --exclude='*.pyc' "$@") || status=$?
  case "$status" in
    0)
      echo "$label:"
      echo "$hits"
      exit 1
      ;;
    1) ;;
    *)
      echo "terminology gate: grep failed with exit status $status while checking $label" >&2
      exit 2
      ;;
  esac
}

public_paths=(
  README.md
  CONTRIBUTING.md
  AGENTS.md
  docs/release-notes/0.11.0-draft.md
  docs/release-notes/0.11.1.md
  docs/src
  python/powerio
  powerio/README.md
  powerio/src
  powerio-core/README.md
  powerio-core/src
  powerio-tx/README.md
  powerio-tx/src
  powerio-dist/README.md
  powerio-dist/src
  powerio-prob/README.md
  powerio-prob/src
  powerio-matrix/README.md
  powerio-matrix/src
  powerio-capi/README.md
  powerio-capi/src
  powerio-capi/include
  powerio-cli/src
  powerio-py/src
)

# Vendored third party files keep the vocabulary of their own publisher and
# are not authored PowerIO text. The only such directory inside the paths
# above is the schema archive under python/powerio/schemas.
vendored_excludes=(--exclude-dir=schemas)

# These words have no defined PowerIO meaning. Protocol specifications and
# third party source data are outside this authored public text.
scan "undefined public terminology" \
  "${vendored_excludes[@]}" -i -e '\b(contracts?|envelopes?)\b' -- "${public_paths[@]}"

# Current user pages use the final domain vocabulary. Historical migration
# and ABI pages intentionally quote names from older releases and are not in
# this list.
current_docs=(
  README.md
  docs/src/README.md
  docs/src/getting-started.md
  docs/src/concepts.md
  docs/src/transmission.md
  docs/src/distribution.md
  docs/src/time-series.md
  docs/src/instances.md
  docs/src/matrices.md
  docs/src/format-fidelity.md
  docs/src/geo-and-display.md
  docs/src/languages.md
  docs/src/python.md
  docs/src/capi.md
  docs/src/cli-mcp.md
  docs/src/corpus-harness.md
  docs/src/pio-json-schema.md
  docs/src/scope-0.11.md
  docs/release-notes/0.11.0-draft.md
  powerio/README.md
  powerio-core/README.md
  powerio-tx/README.md
  powerio-dist/README.md
  powerio-prob/README.md
  powerio-matrix/README.md
  powerio-capi/README.md
)

catch_all='\b(network state|system state|selected state|state inventory|state selector|state export|multi-state|materializ(e|ed|es|ing|ation)|trajector(y|ies)|variant(s)?)\b'
scan "catch-all terminology in current user documentation" \
  -i -e "$catch_all" -- "${current_docs[@]}"

public_report_paths=(
  powerio-cli/src/corpus/mod.rs
  powerio-cli/src/corpus/fingerprint.rs
)
scan "catch-all terminology in public command output" \
  -i -e "$catch_all" -- "${public_report_paths[@]}"

# The public bindings and facade must not regain beta aliases or wrapper
# types. Test helpers and component parser internals are intentionally outside
# this audit.
surface_paths=(
  powerio/src/lib.rs
  powerio/src/value.rs
  powerio-py/src/lib.rs
  python/powerio/__init__.py
  python/powerio/__init__.pyi
  python/powerio/_powerio.pyi
  powerio-capi/src/lib.rs
  powerio-capi/include/powerio.h
)

retired='\b(parse_file|parse_text|parse_str|parse_bytes|write_to|write_string|write_file|to_format|PioValueKind|try_into_typed|IntoTypedModule|StateInventory|StateSelector|SelectedState|list_states|select_state|export_state|materialize_network|PioDcData|DcNetworkData|dc_network_data|dc_data)\b'
scan "retired PowerIO beta API on the public surface" \
  -e "$retired" -- "${surface_paths[@]}"

# Retired diagnostic namespaces must not return. PowerIO IR has one reader for
# its current generation.
scan "retired diagnostic namespace" \
  "${vendored_excludes[@]}" -e '\bLOWER\.[A-Z_]+' -- "${public_paths[@]}"

# Keep the ordinary language quickstarts on the same four operations, each
# naming the case file and nothing else.
grep -Fq 'let module = parse("case9.m")?;' README.md
grep -Fq 'module = powerio.parse("case9.m")' README.md
grep -Fq 'module_ = parse("case9.m")' README.md

echo "terminology: clean"
