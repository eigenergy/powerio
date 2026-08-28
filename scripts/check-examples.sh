#!/usr/bin/env bash
# The example execution gate: every shipped example compiles, and the ones
# with no external input run. C and C++ examples run in ci-mirror's smoke
# jobs against the release library; Julia examples run as Documenter doctests
# in PowerIO.jl; Python guide snippets run in the binding's pytest suite.
set -euo pipefail
cd "$(dirname "$0")/.."

run() { echo "=== $* ==="; "$@"; }

# Every Rust example in the workspace compiles in the feature set it needs.
run cargo build -q --examples -p powerio-tx
run cargo build -q --examples -p powerio --features schema
run cargo build -q --examples -p powerio-dist

# The ones that take no external input run.
run cargo run -q -p powerio --example wasm_smoke --features schema
run cargo run -q -p powerio-tx --example emit -- tests/data/case9.m powermodels >/dev/null
run cargo run -q -p powerio-dist --example regen_bmopf_examples -- --check

# The Go client compiles and runs against the release library when a Go
# toolchain is installed; CI installs one, a dev box without it skips.
if command -v go >/dev/null 2>&1; then
    run cargo build -q -p powerio-capi --release --features dist
    lib_dir="$PWD/target/release"
    case "$(uname -s)" in
        Darwin) lib_path_var=DYLD_LIBRARY_PATH ;;
        *) lib_path_var=LD_LIBRARY_PATH ;;
    esac
    (cd powerio-capi/examples/go-client && \
        run env CGO_CFLAGS="-I$PWD/../../include" CGO_LDFLAGS="-L$lib_dir -lpowerio_capi" \
            "$lib_path_var=$lib_dir" go run . ../../../tests/data/case9.m)
else
    echo "=== go-client: skipped (no go toolchain) ==="
fi

echo "examples OK"
