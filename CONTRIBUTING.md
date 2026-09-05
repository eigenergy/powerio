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
The clippy script covers the same feature combinations CI uses, including the
Python extension and the optional C features.

For the Python bindings, build a wheel from the repository root with
`maturin build --release -o dist`, install it into a virtual environment, and
run `pytest python/tests`. Use the wheel rather than an editable install,
because the `powerio/` crate directory shadows an editable install when pytest
runs from the root. See
[Testing and release checks](https://eigenergy.github.io/powerio/guide/contributor-workflow.html)
for the full procedure.

## C ABI changes

`powerio-capi/include/powerio.h` is generated, so do not edit it by hand.
After any change to a public `pio_*` function, regenerate it:

```
cbindgen --config powerio-capi/cbindgen.toml --crate powerio-capi --output powerio-capi/include/powerio.h
```

CI runs two header checks plus a C smoke test against the built library.
`scripts/capi-header-parity.sh` compares symbol names and runs in every feature
job. `scripts/capi-header-regen.sh` regenerates the header with cbindgen and
diffs it, which is what catches a reordered argument, a changed type, or a
changed struct field; it runs once. A breaking change to an existing `pio_*`
signature bumps `PIO_ABI_VERSION` (in `powerio-capi/src/lib.rs`; the header
`#define` follows from regeneration) and needs a lockstep PowerIO.jl release
that targets the new version. Adding symbols does not bump it. Each ABI
generation gets a changelog entry.

## Releasing

The release version lives in `[workspace.package]` and the workspace dependency
pins in `[workspace.dependencies]`. Update them together; the next Cargo command
updates `Cargo.lock`. Then:

1. Merge the bump with a `CHANGELOG.md` section headed exactly `## X.Y.Z`,
   tag the commit `vX.Y.Z`, and push the tag. The release-binaries workflow
   checks the tag against the workspace version and that heading, builds the
   C ABI tarballs, and stages a draft GitHub release whose body is that
   section.
2. Publish the draft release. The release event fires the PyPI publish
   (python.yml) and the crates.io publish (crates.yml: powerio-core,
   powerio-tx, powerio-dist, powerio-prob, powerio-matrix, powerio, and
   powerio-cli, in dependency order). Both deploy through reviewer protected
   environments (`pypi` and `crates-io`; the protection lives in the repo
   settings). PyPI skips files it already has and crates.io skips versions
   already in the index, so if one of them fails partway you recover by
   running it again.
3. Follow up in PowerIO.jl: regenerate Artifacts.toml from the new tag and
   register the new version (see its CONTRIBUTING.md). A breaking C ABI change
   bumps `PIO_ABI_VERSION` first; see "C ABI changes" above.

## Naming

The [language API guide](https://eigenergy.github.io/powerio/guide/languages.html)
defines the public verbs. `parse` reads a grid exchange representation, `emit`
produces one, and `serialize` and `deserialize` read and write PowerIO IR.
Derived calculations use `calc_*`, and `to_*` is reserved for semantic
transformations.

## Text encoding

Use UTF-8 with LF line endings and no BOM. CI rejects a BOM or cp1252 mojibake
anywhere outside `tests/data`; vendored fixtures keep their committed bytes
exactly as they are. If you work on Windows, configure your editor
accordingly.

## Test fixtures

Use the smallest fixture that exercises the behavior under test. A new fixture
larger than 100 KiB needs explicit maintainer approval for that exact file,
given after you have put its byte count, line count, source, and license in
the pull request. Approval of a feature or test plan is not approval to vendor
its input data. Do not commit material without a license that permits
redistribution.

When source fidelity is itself under test, vendor real cases from the upstream
projects (MATPOWER, pglib) rather than writing bespoke fixtures: record the
source URL, upstream commit, and license, then pin the fixture bytes. For
parser and problem assembly tests, prefer synthetic fixtures. Before you
commit a fixture, run `wc -lc` on it and look at the pull request diff
statistics.

## Documentation prose

Public documentation describes behavior, inputs, outputs, conventions, and
failure conditions before it gets into implementation detail. Keep sentences
about ownership, units, indices, feature gates, and errors short. Use these
terms consistently: network, package, payload, problem instance, source ID,
dense index, operating point, and study commit.

Define an architectural rule once and link to it from everywhere else. Keep
design debate, implementation history, and proposed APIs in issues. Remove
sentences that repeat a heading or add no technical information. Reviewers
enforce these rules as prose; the project does not use an AI detector or
phrase blacklist.

After changing public prose or examples, run `mdbook build docs`,
`mdbook test docs`, and
`RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps`. Regenerate the
committed schemas when their source rustdoc changes, and regenerate
`powerio.h` and run `scripts/capi-header-parity.sh` when C doc comments
change.

## PRs

Use conventional commit subjects (`feat:`, `fix:`, `refactor:`). PRs are
squash merged.

## Tandem changes with PowerIO.jl

The `Julia binding` CI job builds this repository's C ABI and runs PowerIO.jl's test
suite against it. A PR that changes something the two projects share (JSON
shapes, schema versions, `pio_*` behavior) can fail that job against
PowerIO.jl main. When that happens, push a PowerIO.jl branch with the same
name as the powerio branch; the job tests against the companion branch when
one exists. Open both PRs and merge them in either order. Keep PowerIO.jl's
test assertions on the shared pieces at schema strength (same major, shape
present) rather than byte equality, so additive powerio changes do not fail
the tandem job.
