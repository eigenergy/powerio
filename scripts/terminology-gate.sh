#!/usr/bin/env bash
# The 1.0 controlled vocabulary (arch-v1/V1_TERMINOLOGY.md) bans a few words
# from the public register: powerio authors no "interchange" or "exchange
# format", and "contract" never names a wire form or versioning policy. The
# design records under arch-v1/ state the vocabulary and stay out of scope.
set -euo pipefail
cd "$(dirname "$0")/.."

pattern='\bcontract\b|interchange format|exchange format'
paths=(README.md CONTRIBUTING.md CHANGELOG.md AGENTS.md docs/src python/powerio
       powerio/src powerio-core/src powerio-tx/src powerio-dist/src
       powerio-prob/src powerio-matrix/src powerio-capi/src powerio-cli/src
       powerio-py/src)

if hits=$(grep -rn -iE "$pattern" "${paths[@]}" 2>/dev/null); then
  echo "forbidden terminology (see arch-v1/V1_TERMINOLOGY.md):"
  echo "$hits"
  exit 1
fi
echo "terminology: clean"
