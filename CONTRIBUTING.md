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

## Releasing

The release version lives in `[workspace.package]` plus the two intra-workspace
dependency pins in `[workspace.dependencies]`; a bump touches exactly those
three lines of the root Cargo.toml. Then:

1. Merge the bump, tag the commit `vX.Y.Z`, push the tag. The release-binaries
   workflow builds the C ABI tarballs and stages a draft GitHub release.
2. Publish the draft release. The release event fires the PyPI publish
   (python.yml) and the crates.io publish (crates.yml: powerio, powerio-matrix,
   powerio-cli, in dependency order). Both run behind protected environments
   and skip already-uploaded files, so a partial failure is recovered by
   re-running.
3. Follow up in PowerIO.jl: regenerate Artifacts.toml from the new tag and
   register the new version (see its CONTRIBUTING.md). A breaking C ABI change
   bumps `PIO_ABI_VERSION` first; see "C ABI changes" above.

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
