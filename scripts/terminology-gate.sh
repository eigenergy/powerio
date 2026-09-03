#!/usr/bin/env bash
# Keep the PowerIO 0.11 public vocabulary and operation names exact.
set -euo pipefail
cd "$(dirname "$0")/.."

public_paths=(
  README.md
  CONTRIBUTING.md
  AGENTS.md
  docs/release-notes/0.11.0-draft.md
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

# These words have no defined PowerIO meaning. Protocol specifications and
# third party source data are outside this authored public surface.
if hits=$(rg -n -i '\b(contracts?|envelopes?)\b' "${public_paths[@]}" 2>/dev/null); then
  echo "undefined public terminology:"
  echo "$hits"
  exit 1
fi

# Current user pages use the final domain vocabulary. Historical migration
# and ABI pages intentionally quote names from older releases and are not in
# this list.
current_docs=(
  README.md
  docs/src/README.md
  docs/src/getting-started.md
  docs/src/concepts.md
  docs/src/architecture.md
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
  docs/src/api-0.11.md
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
if hits=$(rg -n -i "$catch_all" "${current_docs[@]}" 2>/dev/null); then
  echo "catch-all terminology in current user documentation:"
  echo "$hits"
  exit 1
fi

public_report_paths=(
  powerio-cli/src/corpus/mod.rs
  powerio-cli/src/corpus/fingerprint.rs
)
if hits=$(rg -n -i "$catch_all" "${public_report_paths[@]}" 2>/dev/null); then
  echo "catch-all terminology in public command output:"
  echo "$hits"
  exit 1
fi

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
if hits=$(rg -n "$retired" "${surface_paths[@]}" 2>/dev/null); then
  echo "retired PowerIO beta API on the public surface:"
  echo "$hits"
  exit 1
fi

# Retired diagnostic namespaces must not return. PowerIO IR has one current
# reader and no exceptions for earlier documents.
if hits=$(rg -n '\bLOWER\.[A-Z_]+' "${public_paths[@]}" 2>/dev/null); then
  echo "retired diagnostic namespace:"
  echo "$hits"
  exit 1
fi

# Keep the ordinary language quickstarts on the same four operations, each
# naming the case file and nothing else.
grep -Fq 'let module = powerio::parse("case9.m")?;' README.md
grep -Fq 'module = powerio.parse("case9.m")' README.md
grep -Fq 'module_ = parse("case9.m")' README.md

echo "terminology: clean"
