# Contributing

## Build and test

```
cargo build
cargo test
cargo test -p powerio-capi
cargo fmt --all --check
bash scripts/ci-clippy.sh
```

CI denies clippy warnings and requires Rust code formatted for edition 2024.
The clippy script covers the feature combinations used by CI, including the
Python extension and optional C surfaces.

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

The release version lives in `[workspace.package]` and the workspace dependency
pins in `[workspace.dependencies]`. Update them together; the next Cargo command
updates `Cargo.lock`. Then:

1. Merge the bump, tag the commit `vX.Y.Z`, push the tag. The release-binaries
   workflow builds the C ABI tarballs and stages a draft GitHub release.
2. Publish the draft release. The release event fires the PyPI publish
   (python.yml) and the crates.io publish (crates.yml: powerio, powerio-dist,
   powerio-pkg, powerio-matrix, powerio-prob, powerio-cli, in dependency order). Both deploy
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

When source fidelity is itself under test, vendor real cases from the upstream
projects (MATPOWER, pglib) rather than writing bespoke fixtures: record the
source URL, upstream commit, and license, then pin the fixture bytes. Prefer
synthetic fixtures for parser and problem assembly tests. Before committing a
fixture, run `wc -lc` on it and inspect the pull request diff statistics.

## Documentation prose

Public documentation states behavior, inputs, outputs, conventions, and failure
conditions before implementation detail. Use short sentences for ownership,
units, indices, feature gates, and errors. Use these terms consistently:
network, package, payload, problem instance, source ID, dense index, operating
point, and study commit.

Define an architectural rule once and link to it elsewhere. Keep design debate,
implementation history, and proposed APIs in issues. Remove sentences that
repeat a heading or add no technical information. Reviewers enforce these rules
as prose; the project does not use an AI detector or phrase blacklist.

Run `mdbook build docs`, `mdbook test docs`, and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` after changing
public prose or examples. Regenerate committed schemas when their source
rustdoc changes. Regenerate `powerio.h` and run
`scripts/capi-header-parity.sh` when C doc comments change.

## PRs

Conventional commit subjects (`feat:`, `fix:`, `refactor:`); squash merge.

## Tandem changes with PowerIO.jl

The `Julia binding` CI job builds this repository's C ABI and runs PowerIO.jl's test
suite against it. A PR that moves the shared surface (JSON shapes, schema
versions, `pio_*` behavior) can fail that job against PowerIO.jl main. Push a
PowerIO.jl branch with the **same name** as the powerio
branch; the job tests against the companion branch when it exists. Open both
PRs, merge in either order, and keep PowerIO.jl test assertions on the shared
surface at schema strength (same major, shape present) rather than byte
equality, so additive powerio changes do not fail the tandem job.
