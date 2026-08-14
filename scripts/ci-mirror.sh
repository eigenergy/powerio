#!/usr/bin/env bash
# Everything rust.yml runs, in the same feature combinations.
#
# `cargo test --workspace` is not this: it builds powerio-capi with default
# features only, so every test behind `arrow`, `gridfm`, `matrix` or `prob` is
# skipped, and powerio-py is excluded entirely on a machine without the Python
# library to link against.
set -euo pipefail
cd "$(dirname "$0")/.."

run() { echo "=== $* ==="; "$@"; }

run cargo fmt --all --check
run ./scripts/ci-clippy.sh
run ./scripts/capi-header-parity.sh

# The build job documents the workspace under -D warnings, so a doc comment can
# fail CI while every test passes. An intra-doc link from a public item to a
# private one is the easy way to do it.
RUSTDOCFLAGS="-D warnings" run cargo doc --workspace --no-deps

run cargo test -p powerio -p powerio-matrix -p powerio-prob -p powerio-cli \
    -p powerio-capi -p powerio-dist -p powerio-pkg
run cargo test -p powerio-prob --features matrix
run cargo test -p powerio-matrix --features gridfm

# The four powerio-capi combinations rust.yml builds.
run cargo test -p powerio-capi --no-default-features
run cargo test -p powerio-capi --features arrow
run cargo test -p powerio-capi --features dist
run cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,pkg,prob

# The C smoke test in the two configurations that exercise the most surface.
# `cargo test` never compiles it, so a stale assertion in it reaches CI intact.
smoke_dir=$(mktemp -d)
trap 'rm -rf "$smoke_dir"' EXIT
lib_path_var=DYLD_LIBRARY_PATH
[ "$(uname -s)" = "Linux" ] && lib_path_var=LD_LIBRARY_PATH

run cargo build -q -p powerio-capi --release --features dist
cc -DPIO_DIST -I powerio-capi/include powerio-capi/examples/smoke.c \
   -L target/release -lpowerio_capi -o "$smoke_dir/smoke_dist"
run env "$lib_path_var=target/release" "$smoke_dir/smoke_dist" tests/data/case9.m

run cargo build -q -p powerio-capi --release --features arrow,matrix,gridfm,dist,pkg,prob
cc -DPIO_ARROW -DPIO_MATRIX -DPIO_GRIDFM -DPIO_DIST -DPIO_PKG -DPIO_PROB \
   -I powerio-capi/include powerio-capi/examples/smoke.c \
   -L target/release -lpowerio_capi -o "$smoke_dir/smoke_release"
run cargo run -q -p powerio-cli -- gridfm tests/data/case9.m -o "$smoke_dir/gridfm"
run env "$lib_path_var=target/release" "$smoke_dir/smoke_release" \
    tests/data/case9.m "$smoke_dir/gridfm/case9/raw"

# The generated schema must already match what the example emits.
run cargo run -q -p powerio-pkg --example generate_schemas --features schema -- docs/schema
if ! git diff --quiet -- docs/schema; then
  echo "error: docs/schema is stale; commit the regenerated files" >&2
  exit 1
fi

echo "=== all green ==="
