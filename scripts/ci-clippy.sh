#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage: bash scripts/ci-clippy.sh [target]

Targets:
  all             Run every clippy target below.
  workspace       Root Rust CI clippy for non extension crates.
  matrix-gridfm   powerio-matrix with the gridfm feature.
  capi-no-default powerio-capi with no default features.
  capi-arrow      powerio-capi with arrow.
  capi-release    powerio-capi with arrow,matrix,gridfm,dist,pkg.
  capi-dist       powerio-capi with dist.
  powerio-py      PyO3 extension with extension-module,gridfm.

Run `all` before pushing changes that affect Rust, C ABI, Arrow, matrices,
features, or the Python extension.
EOF
}

run() {
  printf '\n'
  printf '+'
  printf ' %q' "$@"
  printf '\n'
  "$@"
}

target="${1:-all}"

case "$target" in
  all)
    bash "$0" workspace
    bash "$0" matrix-gridfm
    bash "$0" capi-no-default
    bash "$0" capi-arrow
    bash "$0" capi-release
    bash "$0" capi-dist
    bash "$0" powerio-py
    ;;
  workspace)
    run cargo clippy --all-targets \
      -p powerio \
      -p powerio-matrix \
      -p powerio-opf \
      -p powerio-cli \
      -p powerio-capi \
      -p powerio-dist \
      -p powerio-pkg \
      -- -D warnings
    ;;
  matrix-gridfm)
    run cargo clippy -p powerio-matrix --all-targets --features gridfm -- -D warnings
    ;;
  capi-no-default)
    run cargo clippy -p powerio-capi --all-targets --no-default-features -- -D warnings
    ;;
  capi-arrow)
    run cargo clippy -p powerio-capi --all-targets --features arrow -- -D warnings
    ;;
  capi-release)
    run cargo clippy -p powerio-capi --all-targets --features arrow,matrix,gridfm,dist,pkg -- -D warnings
    ;;
  capi-dist)
    run cargo clippy -p powerio-capi --all-targets --features dist -- -D warnings
    ;;
  powerio-py)
    run cargo clippy -p powerio-py --no-deps --features extension-module,gridfm -- -D warnings
    ;;
  -h|--help|help)
    usage
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac
