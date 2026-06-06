#!/usr/bin/env bash
# Fetch the large benchmark cases into tests/data/large (gitignored, kept out of
# the repo). BSD-3. MATPOWER cases from the MATPOWER repo; case99k/case193k from
# goghino/opf_benchmarks. case193k is ~54 MB.
set -euo pipefail
mp=https://raw.githubusercontent.com/MATPOWER/matpower/master/data
gg=https://raw.githubusercontent.com/goghino/opf_benchmarks/master/cases
dir="$(cd "$(dirname "$0")/.." && pwd)/tests/data/large"
mkdir -p "$dir"
for c in case9241pegase case13659pegase \
         case_ACTIVSg2000 case_ACTIVSg10k case_ACTIVSg25k case_ACTIVSg70k \
         case_SyntheticUSA; do
  curl -fsSL "$mp/$c.m" -o "$dir/$c.m" && echo "fetched $c"
done
for c in case99k case193k; do
  curl -fsSL "$gg/$c.m" -o "$dir/$c.m" && echo "fetched $c"
done
