# Contributing

## Build and test

```
cargo build
cargo test -p powerio -p powerio-matrix -p powerio-cli -p powerio-capi
cargo fmt --all --check
cargo clippy --all-targets -p powerio -p powerio-matrix -p powerio-cli -p powerio-capi -- -D warnings
```

CI denies clippy warnings and requires rustfmt-clean code (edition 2024 style).
The gridfm Parquet export is feature gated; exercise it with
`cargo test -p powerio-matrix --features gridfm`.

Python bindings: `maturin develop` against `powerio-py`, then
`pytest python/tests`. See [docs/python.md](docs/python.md).

## C ABI changes

`powerio-capi/include/powerio.h` is generated; never hand-edit it. After any
change to the `powerio-capi` surface:

```
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi --output powerio-capi/include/powerio.h
```

CI checks header/source symbol parity and runs a C smoke test against the
built library. A breaking change to an existing `pio_*` signature bumps
`PIO_ABI_VERSION` (in `powerio-capi/src/lib.rs`; the header `#define` follows
from regeneration) and requires a lockstep PowerIO.jl release targeting the new
version. Additive symbols don't bump it. The history is in
[powerio-capi/README.md](powerio-capi/README.md).

## Naming

The cross-language verb taxonomy lives in [docs/languages.md](docs/languages.md):
`parse_file` / `parse_str` / `from_json` produce a `Network`, `to_*` derive from
it, `convert_file` goes file to text in one call. A Network is the parsed model;
a case is the file it came from.

## Text encoding

UTF-8 with LF line endings, no BOM. CI rejects BOM and cp1252 mojibake outside
`tests/data`; vendored fixtures keep their committed bytes exactly. Configure
Windows editors accordingly.

## Test fixtures

Vendor real cases from the upstream projects (MATPOWER, pglib) rather than
writing bespoke fixtures; pin their bytes.

## PRs

Conventional commit subjects (`feat:`, `fix:`, `refactor:`); squash merge.
