#!/usr/bin/env bash
# Build the C ABI and print the env line that points a consumer at it.
#
# With a sibling ../PowerIO.jl checkout, PowerIO.jl auto-discovers this build and
# needs nothing further. This script is for non-sibling layouts and for the wider
# C/C++/Julia FFI: run it, then `eval "$(scripts/dev-capi.sh)"` to export the path.
set -euo pipefail
cd "$(dirname "$0")/.."

# --features arrow so the sibling PowerIO.jl build gets pio_export_arrow; the
# base ABI is identical with or without it.
cargo build -p powerio-capi --release --features arrow >&2

case "$(uname -s)" in
  Darwin) ext=dylib ;;
  *)      ext=so ;;
esac
lib="$PWD/target/release/libpowerio_capi.$ext"

echo "built $lib" >&2
echo "export POWERIO_CAPI=$lib"
