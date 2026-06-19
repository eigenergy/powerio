# Releasing powerio (v0.3.0, with powerio-dist)

Run after `release-prep-v0.3.0` merges to `main`. The workspace version is a
single source (`[workspace.package].version`); all crates inherit it, so a
release is one version bump plus an ordered publish.

## 1. Version bump

Bump `[workspace.package].version` and the three intra-workspace dependency pins
in the root `Cargo.toml` from `0.2.4` to `0.3.0` in lockstep:

```toml
[workspace.package]
version = "0.3.0"

[workspace.dependencies]
powerio       = { path = "powerio",       version = "0.3.0" }
powerio-matrix = { path = "powerio-matrix", version = "0.3.0" }
powerio-dist  = { path = "powerio-dist",  version = "0.3.0" }
```

`powerio-dist` inherits via `version.workspace = true`. Update `CHANGELOG.md`,
run `cargo build` to refresh `Cargo.lock`, and confirm `pio_abi_version()` is 4
(unchanged — the dist surface is additive under the same ABI version).

## 2. crates.io publish order

Four crates publish to crates.io, in dependency order; `powerio-capi` and
`powerio-py` are `publish = false` (they ship as release tarballs and a PyPI
wheel). Dry-run each first, publish, then wait for the index before the next:

```
cargo publish -p powerio        --locked   # core, no workspace deps
cargo publish -p powerio-matrix --locked   # depends on powerio
cargo publish -p powerio-dist   --locked   # leaf (serde/serde_json/thiserror only)
cargo publish -p powerio-cli    --locked   # depends on powerio-matrix, powerio-dist
```

`powerio-dist` has no intra-workspace dependencies, so its only ordering
constraint is that it precede `powerio-cli` (which depends on it). Verify it is
self-contained first: `cargo publish -p powerio-dist --dry-run --locked`.

## 3. Platform artifacts and the C ABI

`release-binaries.yml` builds the `powerio-capi` cdylib for the five PowerIO.jl
platforms with `--features arrow,gridfm,dist`, so the released `.dylib`/`.so`/
`.dll` exports the `pio_dist_*` distribution surface. PowerIO.jl probes it at
runtime with `pio_has_feature("dist")`; no ABI-version change is needed (the dist
surface is additive under `PIO_ABI_VERSION = 4`). The cbindgen header parity job
gates the committed `powerio.h`.

## 4. Tag and publish

Tag `v0.3.0` and push; CI builds the wheels and platform tarballs and stages a
draft GitHub release for a maintainer to publish (publishing fires the crates.io
and PyPI steps). `powerio-py` ships as a maturin wheel to PyPI with the
`extension-module,gridfm` features; the `dist` surface is always compiled into
the Python extension.

## Deferred (additive, post-0.3.0)

- A lossless `powerio-dist-json` snapshot (serde on `DistNetwork`) with a payload
  `meta.version`, the distribution analogue of `powerio-json`, plus a CI gate
  requiring `#[serde(default)]` on every new dist field. The dist C signatures
  are frozen under v4; this snapshot is the place a future non-additive schema
  change is versioned, in the payload, never in the ABI. Tracked for v0.3.x.
- A `meta`/`$schema` block on BMOPF output is blocked by the schema's
  `additionalProperties: false`; it depends on the task-force extension-hatch
  decision (PR #82 discussion), not on powerio.
