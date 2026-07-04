#!/usr/bin/env bash
# Build and run the C ABI smoke programs against the local release library.
set -euo pipefail
cd "$(dirname "$0")/.."

tmp="${TMPDIR:-/tmp}/powerio-capi-smoke-$$"
mkdir -p "$tmp"
trap 'rm -rf "$tmp"' EXIT

cargo build -p powerio-capi --release --features arrow,matrix,gridfm,dist,pkg
cargo run -p powerio-cli -- gridfm tests/data/case9.m -o "$tmp/gridfm" >/dev/null
cp tests/data/case9.m "$tmp/case9.m"

case "$(uname -s)" in
    Darwin) lib_env=DYLD_LIBRARY_PATH ;;
    *)      lib_env=LD_LIBRARY_PATH ;;
esac

if [ -n "${!lib_env:-}" ]; then
    lib_path="$PWD/target/release:${!lib_env}"
else
    lib_path="$PWD/target/release"
fi

cc -DPIO_ARROW -DPIO_GRIDFM -DPIO_DIST -DPIO_PKG \
   -I powerio-capi/include powerio-capi/examples/smoke.c \
   -L target/release -lpowerio_capi -o "$tmp/pio_smoke"
env "$lib_env=$lib_path" "$tmp/pio_smoke" \
   "$tmp/case9.m" "$tmp/gridfm/case9/raw"

c++ -std=c++17 -DPIO_ARROW -DPIO_GRIDFM -DPIO_DIST -DPIO_PKG \
    -I powerio-capi/include powerio-capi/examples/header_cpp.cpp \
    -L target/release -lpowerio_capi -o "$tmp/pio_header_cpp"
env "$lib_env=$lib_path" "$tmp/pio_header_cpp"

echo "C ABI smoke checks passed"
