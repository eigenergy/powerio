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

# Retired diagnostic namespaces must not survive in prose or doc comments.
# LOWER.* became TRANSFORM.*; only the legacy09 module, which documents the
# 0.9 wire form, may still spell the old token.
retired='\bLOWER\.[A-Z_]+'
if hits=$(grep -rn -E "$retired" "${paths[@]}" 2>/dev/null | grep -v legacy09); then
  echo "retired diagnostic namespace (see arch-v1/V1_TERMINOLOGY.md):"
  echo "$hits"
  exit 1
fi

# The 0.9 `powerio package` CLI subcommand and its MCP/language encoding
# (package_json, the package transport, to_package/from_package/read_package/
# write_package) are retired in favor of `powerio module` and its module_json
# encoding. "powerio package" as a command is matched at the start of a line
# or inside backticks, so ordinary prose ("the powerio package on PyPI")
# stays clear. History pages that document the rename, and CHANGELOG.md's
# older entries, may still name the retired forms.
package='(^|`)powerio package\b|\bpackage_json\b|transport="package"|transport = "package"|\b(to_package|from_package|read_package|write_package)\b'
package_history='retired-names\.md|migration-v1\.md|migration\.md|migration-v0\.9\.md|migration-v0\.7\.md|CHANGELOG\.md'
if hits=$(grep -rn -E "$package" "${paths[@]}" 2>/dev/null | grep -vE "$package_history"); then
  echo "leaked 0.9 package vocabulary (see arch-v1/V1_TERMINOLOGY.md):"
  echo "$hits"
  exit 1
fi
echo "terminology: clean"
