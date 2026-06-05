#!/usr/bin/env bash
# Fetch the large MATPOWER cases for the benchmark into tests/data/large
# (gitignored, kept out of the repo). BSD-3, from the MATPOWER repo.
set -euo pipefail
base=https://raw.githubusercontent.com/MATPOWER/matpower/master/data
dir="$(cd "$(dirname "$0")/.." && pwd)/tests/data/large"
mkdir -p "$dir"
for c in case9241pegase case13659pegase case_ACTIVSg2000; do
  curl -fsSL "$base/$c.m" -o "$dir/$c.m" && echo "fetched $c"
done
