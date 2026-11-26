#!/usr/bin/env bash
set -euo pipefail

destination=${MATPOWER_PATH:?Set MATPOWER_PATH to the directory for MATPOWER 8.1}
if [[ -f "$destination/lib/runpf.m" ]]; then
    exit 0
fi
archive=$(mktemp)
trap 'rm -f "$archive"' EXIT
curl -fsSL --retry 3 https://codeload.github.com/MATPOWER/matpower/tar.gz/refs/tags/8.1 -o "$archive"
printf '%s  %s\n' 5f08ced1a32e8d0d658ee54b73678c7364a899bae479882f2bc419e91442b69f "$archive" | shasum -a 256 -c -
mkdir -p "$destination"
tar -xzf "$archive" --strip-components=1 -C "$destination"
