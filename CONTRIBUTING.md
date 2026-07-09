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
`pytest python/tests`. See the
[Python guide](https://eigenergy.github.io/powerio/guide/python.html).

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
`powerio-capi/README.md`.

## Releasing

The release version lives in `[workspace.package]` plus the four workspace
dependency pins in `[workspace.dependencies]`; a bump touches exactly those five
lines of the root Cargo.toml (Cargo.lock follows on the next build). Then:

1. Merge the bump, tag the commit `vX.Y.Z`, push the tag. The release-binaries
   workflow builds the C ABI tarballs and stages a draft GitHub release.
2. Publish the draft release. The release event fires the PyPI publish
   (python.yml) and the crates.io publish (crates.yml: powerio, powerio-dist,
   powerio-pkg, powerio-matrix, powerio-cli, in dependency order). Both deploy
   through reviewer protected environments (`pypi`, `crates-io`; the protection
   lives in the repo settings). PyPI skips already-uploaded files and crates.io
   skips versions already in the index, so a partial failure is recovered by
   re-running.
3. Follow up in PowerIO.jl: regenerate Artifacts.toml from the new tag and
   register the new version (see its CONTRIBUTING.md). A breaking C ABI change
   bumps `PIO_ABI_VERSION` first; see "C ABI changes" above.

## Naming

The cross-language verb taxonomy lives in the
[language API guide](https://eigenergy.github.io/powerio/guide/languages.html):
`parse_file` / `parse_str` / `from_json` produce a `Network`, `to_*` derive from
it, `convert_file` goes file to text in one call. A Network is the parsed model;
a case is the file it came from.

## Text encoding

UTF-8 with LF line endings, no BOM. CI rejects BOM and cp1252 mojibake outside
`tests/data`; vendored fixtures keep their committed bytes exactly. Configure
Windows editors accordingly.

## Test fixtures

Use the smallest fixture that exercises the behavior under test. New fixtures
larger than 100 KiB require explicit maintainer approval for that exact file
after its byte count, line count, source, and license are stated in the pull
request. Approval of a feature or test plan is not approval to vendor its input
data. Do not commit material without a license that permits redistribution.

Prefer synthetic fixtures for parser and problem assembly tests. Use licensed
upstream cases only when source fidelity is itself under test. Before committing
a fixture, run `wc -lc` on it and inspect the pull request diff statistics.

## PRs

Conventional commit subjects (`feat:`, `fix:`, `refactor:`); squash merge.

## Tandem changes with PowerIO.jl

The `Julia binding` CI job builds this repo's C ABI and runs PowerIO.jl's test
suite against it. A PR that moves the shared surface (JSON shapes, schema
versions, `pio_*` behavior) fails that job against PowerIO.jl main by
construction. Push a PowerIO.jl branch with the **same name** as the powerio
branch; the job tests against the companion branch when it exists. Open both
PRs, merge in either order, and keep PowerIO.jl test assertions on the shared
surface at schema strength (same major, shape present) rather than byte
equality, so additive powerio changes do not fail the tandem job.
