#!/usr/bin/env bash
# Fetch the ACTIVSg2000 sibling format set (aux, pwb, pwd, RAW, EPC, m) into
# tests/data/large/ACTIVSg2000 (gitignored). The TAMU Electric Grid Test Case
# Repository (https://electricgrids.engr.tamu.edu/) gates current downloads
# behind a form; this pulls the June 2016 public archive it links directly.
# Tests and benchmarks that use these files skip when the directory is absent.
set -euo pipefail
id="1tOIK_RVQaZZDo_oIi75bVdPsAlQ7J1l9"
sha="82c25e3fbae6a9d1d8aab42b1cc857b8dff3db60127ef0fa43eee4dc8e208ba7"
dir="$(cd "$(dirname "$0")/.." && pwd)/tests/data/large/ACTIVSg2000"
mkdir -p "$dir"
zip="$dir/ACTIVSg2000_June2016.zip"
if [ ! -f "$zip" ]; then
  curl -fsSL "https://drive.google.com/uc?export=download&id=$id" -o "$zip"
fi
echo "$sha  $zip" | shasum -a 256 -c -
unzip -o -j -q "$zip" -d "$dir"
ls -l "$dir"
