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

# Current user documentation names the object or field in question. These
# generic words obscure whether the text means retained source, module
# records, a calculation, or a stored document. Developer Guides and frozen
# history remain outside this gate because they quote earlier releases.
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
  docs/src/beta-scope.md
)
generic='\b(envelope|provenance|study)\b'
if hits=$(grep -n -iE "$generic" "${current_docs[@]}" 2>/dev/null); then
  echo "generic terminology in current user documentation:"
  echo "$hits"
  exit 1
fi

if hits=$(grep -n -F 'PowerIO.parse(' "${current_docs[@]}" 2>/dev/null); then
  echo "Julia examples must use parse_file() after using PowerIO:"
  echo "$hits"
  exit 1
fi

# Keep the three ordinary language quickstarts executable and stable.
grep -Fq 'let module: PioModule<BalancedNetwork> = powerio::try_into_typed(module)?;' README.md
grep -Fq 'module = powerio.parse("case9.m")' README.md
grep -Fq 'module_ = parse_file("case9.m")' README.md
echo "terminology: clean"
