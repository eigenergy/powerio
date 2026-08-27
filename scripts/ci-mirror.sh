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
run ./scripts/capi-header-regen.sh

# A Windows editor has twice corrupted text in a PR: a UTF-8 BOM, and UTF-8
# re-read as cp1252. tests/data is exempt; vendored fixtures keep their bytes.
echo "=== encoding ==="
if git grep -Iln $'\xef\xbb\xbf' -- .; then
  echo "UTF-8 BOM found in the files above" >&2; exit 1
fi
if git grep -Iln $'\xc3\xa2' -- ':!tests/data'; then
  echo "mojibake found in the files above" >&2; exit 1
fi

# The build job documents the workspace under -D warnings, so a doc comment can
# fail CI while every test passes. An intra-doc link from a public item to a
# private one is the easy way to do it. --all-features, matching docs.yml: a
# doc comment behind arrow, gridfm, matrix or prob is unreachable without it.
RUSTDOCFLAGS="-D warnings" run cargo doc --workspace --no-deps --all-features

# powerio-prob sits below the matrix crate: it may reach the model crates
# (crate_graph.rs states and tests the whole layout) but never powerio-matrix.
# Spelled as if/exit because a `!`-prefixed pipeline never trips errexit,
# which left the old spelling decorative.
echo "=== dependency boundaries ==="
cargo check -q -p powerio-prob --no-default-features
if cargo tree -p powerio-prob --no-default-features --edges normal | grep -q powerio-matrix; then
  echo "error: powerio-prob reaches powerio-matrix" >&2; exit 1
fi

# The parser crates have to stay free of anything that needs an OS, so the
# readers can run in a browser. Skipped rather than failing when the target
# is not installed (rustup target add wasm32-unknown-unknown).
if rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  run cargo check --target wasm32-unknown-unknown -p powerio -p powerio-tx -p powerio-dist
else
  echo "=== skipped: wasm32-unknown-unknown target not installed ==="
fi

run cargo test -p powerio -p powerio-tx -p powerio-core -p powerio-matrix -p powerio-prob -p powerio-cli \
    -p powerio-capi -p powerio-dist
run cargo test -p powerio --features matrix
run cargo test -p powerio --features gridfm
run cargo test -p powerio-matrix --features gridfm

# The four powerio-capi combinations rust.yml builds.
run cargo test -p powerio-capi --no-default-features
run cargo test -p powerio-capi --features arrow
run cargo test -p powerio-capi --features dist
run cargo test -p powerio-capi --features arrow,matrix,gridfm,dist,prob

# The C smoke test and the C++ header check, in the two configurations that
# exercise the most surface. `cargo test` compiles neither, so a stale
# assertion in either reaches CI intact.
smoke_dir=$(mktemp -d)
trap 'rm -rf "$smoke_dir"' EXIT
lib_path_var=DYLD_LIBRARY_PATH
[ "$(uname -s)" = "Linux" ] && lib_path_var=LD_LIBRARY_PATH

run cargo build -q -p powerio-capi --release --features dist
cc -DPIO_DIST -I powerio-capi/include powerio-capi/examples/smoke.c \
   -L target/release -lpowerio_capi -o "$smoke_dir/smoke_dist"
run env "$lib_path_var=target/release" "$smoke_dir/smoke_dist" tests/data/case9.m
c++ -std=c++17 -DPIO_DIST -I powerio-capi/include powerio-capi/examples/header_cpp.cpp \
   -L target/release -lpowerio_capi -o "$smoke_dir/header_cpp_dist"
run env "$lib_path_var=target/release" "$smoke_dir/header_cpp_dist"

run cargo build -q -p powerio-capi --release --features arrow,matrix,gridfm,dist,prob
cc -DPIO_ARROW -DPIO_MATRIX -DPIO_GRIDFM -DPIO_DIST -DPIO_PKG -DPIO_PROB \
   -I powerio-capi/include powerio-capi/examples/smoke.c \
   -L target/release -lpowerio_capi -o "$smoke_dir/smoke_release"
c++ -std=c++17 -DPIO_ARROW -DPIO_MATRIX -DPIO_GRIDFM -DPIO_DIST -DPIO_PKG -DPIO_PROB \
   -I powerio-capi/include powerio-capi/examples/header_cpp.cpp \
   -L target/release -lpowerio_capi -o "$smoke_dir/header_cpp_release"
run env "$lib_path_var=target/release" "$smoke_dir/header_cpp_release"
run cargo run -q -p powerio-cli -- gridfm tests/data/case9.m -o "$smoke_dir/gridfm"
run env "$lib_path_var=target/release" "$smoke_dir/smoke_release" \
    tests/data/case9.m "$smoke_dir/gridfm/case9/raw"

# The generated schema must already match what the example emits. Checked with
# `git status --porcelain`, which also sees the new directory a version bump
# creates; a diff of tracked files would report nothing while it goes
# uncommitted.
run cargo run -q -p powerio --example generate_schemas --features schema -- docs/schema
if [ -n "$(git status --porcelain -- docs/schema)" ]; then
  git status --porcelain -- docs/schema
  echo "error: docs/schema is stale; commit the regenerated files" >&2
  exit 1
fi

# crates.yml packages every publishable crate and audits each archive for the
# license files a published crate must carry. The verify builds compile the freshly packaged siblings from cargo's overlay
# registry, whose unpacks carry fixed tarball timestamps: a stale unpack or a
# stale compiled artifact of the same name and version is fingerprint valid
# forever and shadows new API with errors that do not exist in the tree. CI
# runners start clean; a developer machine must drop the overlay unpacks and
# give the packaging its own disposable build directory (which also keeps the
# audit off leftover archives).
rm -rf "$HOME"/.cargo/registry/src/-*/powerio-0.* "$HOME"/.cargo/registry/src/-*/powerio-[a-z]*-0.* target/package-verify
run env CARGO_TARGET_DIR=target/package-verify cargo package --workspace --exclude powerio-capi --exclude powerio-py --allow-dirty
run python3 scripts/audit-release-archives.py target/package-verify/package/*.crate

# fuzz.yml builds the detached fuzz workspace with cargo-fuzz on nightly; a
# plain check catches source breakage (a renamed entry point, a moved type)
# without either.
run cargo check --manifest-path fuzz/Cargo.toml

# docs.yml runs both, and neither is reachable from cargo: an unannotated code
# fence in the book is compiled as a Rust doctest, so a naming pattern or a
# shell snippet fails a job nothing else here covers. Skipped when mdbook is
# absent rather than failing a run that is otherwise complete.
if command -v mdbook >/dev/null 2>&1; then
  run mdbook build docs --dest-dir target/doc/guide
  run mdbook test docs
else
  echo "=== skipped: mdbook not installed (cargo install mdbook) ==="
fi

echo "=== all green ==="
