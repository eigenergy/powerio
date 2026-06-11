#!/usr/bin/env bash
# Fetch the ACTIVSg2000 sibling format sets into tests/data/large/ACTIVSg2000
# (gitignored). Two sources, two case vintages:
#
# - The June 2016 archive (aux, pwb, pwd, RAW, EPC, m, all exported the same
#   day from one case): the TAMU Electric Grid Test Case Repository
#   (https://electricgrids.engr.tamu.edu/) gates current downloads behind a
#   form, but links this archive directly. Same day exports make it the cross
#   format parity oracle.
# - ACTIV_SG_2000_v19.pwb/.pwd, a later revision of the same case saved in the
#   Simulator 19 file format, hosted by PowerWorld Corporation at
#   https://www.powerworld.com/new-synthetic-power-flow-cases ("publicly
#   available, non-confidential synthetic" cases; no license text given, so
#   the files are fetched from the source, never vendored). A second writer
#   vintage exercises the .pwb reader's version handling.
#
# Tests and benchmarks that use these files skip when the directory is absent.
set -euo pipefail
dir="$(cd "$(dirname "$0")/.." && pwd)/tests/data/large/ACTIVSg2000"
mkdir -p "$dir"

id="1tOIK_RVQaZZDo_oIi75bVdPsAlQ7J1l9"
sha="82c25e3fbae6a9d1d8aab42b1cc857b8dff3db60127ef0fa43eee4dc8e208ba7"
zip="$dir/ACTIVSg2000_June2016.zip"
if [ ! -f "$zip" ]; then
  curl -fsSL "https://drive.google.com/uc?export=download&id=$id" -o "$zip"
fi
echo "$sha  $zip" | shasum -a 256 -c -
unzip -o -j -q "$zip" -d "$dir"

pw=https://www.powerworld.com/files
for f in ACTIV_SG_2000_v19.pwb ACTIV_SG_2000_v19.pwd; do
  [ -f "$dir/$f" ] || curl -fsSL "$pw/$f" -o "$dir/$f"
done
shasum -a 256 -c - <<EOF
b2ba4bbf3c57408a9791dfaad9d03a91acac374fdfc870563329b60745d7a30b  $dir/ACTIV_SG_2000_v19.pwb
a9f545c33beb65f68c08f8b3811a34317b804fa14cb07e21e6b865a5528016d3  $dir/ACTIV_SG_2000_v19.pwd
EOF

# The published ACTIVSg2000 case in MATPOWER format, from the MATPOWER
# repository (BSD 3-clause): the value oracle for the v19 .pwb, which has no
# same day sibling exports.
mp="https://raw.githubusercontent.com/MATPOWER/matpower/master/data"
f="case_ACTIVSg2000.m"
[ -f "$dir/$f" ] || curl -fsSL "$mp/$f" -o "$dir/$f"
echo "8d00618de8fd10bf35a599f59d2deebfecd0d86e28fcff73219ad7c4ebab860b  $dir/$f" | shasum -a 256 -c -
ls -l "$dir"
