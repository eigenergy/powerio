#!/usr/bin/env bash
set -euo pipefail

# Official PNNL GridLAB-D 5.3 binary for the Linux x86-64 GitHub runner.
destination=${1:?Pass an installation directory for GridLAB-D}
if [[ -x "$destination/bin/gridlabd" ]]; then
    exit 0
fi
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
curl -fsSL --retry 3 https://github.com/gridlab-d/gridlab-d/releases/download/v5.3.0/GridLAB-D-5.3.0-Linux.tar.gz -o "$archive"
printf '%s  %s\n' 160cff7ae04ec769ed89e6fa607f10907ff8a7868c22f8b4d6fa1ed2a5c91b6c "$archive" | shasum -a 256 -c -
mkdir -p "$destination"
tar -xzf "$archive" --strip-components=1 -C "$destination"
