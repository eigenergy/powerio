# CLAUDE.md

Guidance for Claude Code working in this repo.

## Purpose

A Cargo workspace of three Rust crates plus a Python package. Parses power
network case files, converts losslessly between formats, and emits sparse
matrices and graph views for any downstream solver. (Planned) feeds the GridFM
ML pipeline.

- **`caseio`** — the parser, typed `MpcCase`, the format-neutral `Network` hub,
  the lossless writer, and the format converters. Light deps (thiserror,
  num-complex, petgraph, serde, serde_json, fast-float); no matrix or TUI stack.
- **`casemat`** — sparse matrices and graph views built on `caseio` (which it
  re-exports), plus the `casemat` CLI/TUI.
- **`casemat-ext`** — PyO3 extension behind the `casemat` Python package
  (`python/casemat/`); hands back COO triplets that scipy assembles.

Formats. Readers: MATPOWER `.m`, PowerModels JSON, PSS/E `.raw` (v33),
PowerWorld `.aux`. Writers: those plus EGRET JSON. Every format meets at
`Network`, so a new format is one reader/writer at the hub, not a pairwise
converter.

Matrix outputs (casemat):
- B' (FDPF, shuntless). Singular positive Laplacian, rank n-1.
- B'' (FDPF, with shunts and taps). SDDM when bus shunts are present.
- `Re(Y_bus)`, `-Im(Y_bus)` (full).
- LACPF block `[[G, -B], [-B, -G]]` (linear AC power flow, flat start, 2n×2n, indefinite).
- Adjacency (`MatrixKind::Adjacency`); PTDF and LODF (DC sensitivities, `sensitivities` subcommand).
- DC-OPF instance bundle (`dcopf` subcommand, `opf_pipeline::write_dcopf_bundle`): signed incidence `A` (n×m), branch susceptance `b`, weighted Laplacian `L = A diag(b) Aᵀ` and its slack-grounded form, flow map `B Aᵀ`, generator cost `Q`/`c`, bounds, thermal limits `f̄`, generator→bus `C_g`, nodal load `p_d`, `e_r`.
- petgraph `UnGraph<bus_idx, branch_idx>` view + connectivity / radial diagnostics.
- Planned: LinDist3Flow, Parquet (gridfm-datakit schema).

## Commands

```
cargo build --release        # caseio + casemat (default-members)
cargo test                   # caseio + casemat
cargo clippy --all-targets

# casemat CLI (binary is `casemat`):
casemat                                                   # TUI
casemat batch -i tests/data -o out --matrices bprime,bdoubleprime
casemat gen --topology lattice --n 1024 -o out
casemat verify tests/data/case30.m --kind bdoubleprime
casemat dcopf tests/data/case30.m -o out
casemat sensitivities tests/data/case30.m -o out
casemat convert tests/data/case14.m --to psse -o case14.raw

# Python bindings (PyO3 crate needs libpython, so it is NOT in default-members):
cargo build -p casemat-ext   # plain cargo build of the extension
maturin develop              # build + install into the active venv
pytest python/tests
```

## Layout

```
caseio/                       # parser + Network hub + converters
├── src/lib.rs               # public re-exports
├── src/error.rs             # thiserror Error
├── src/case.rs              # MpcCase, Bus, Branch, Generator, GenCost,
│                            #   Storage, DcLine, ConnectivityReport
│                            #   + petgraph view: to_petgraph, is_radial,
│                            #     n_connected_components, connectivity_report
├── src/network.rs           # Network, SourceFormat (format-neutral hub)
├── src/parser/
│   ├── mod.rs               # format dispatcher
│   └── matpower/            # parse_matpower(_file), tokens, matlab, locate,
│                            #   writer (the lossless source-retaining path)
├── src/format/             # converters at the hub
│   ├── mod.rs              # write_as, TargetFormat, Conversion
│   ├── powermodels.rs     # PowerModels JSON reader + writer
│   ├── psse.rs            # PSS/E .raw reader + writer
│   ├── powerworld.rs      # PowerWorld .aux reader + writer
│   └── egret.rs           # EGRET JSON writer
└── tests/                  # convert, roundtrip, roundtrip_formats

casemat/                      # matrices + CLI/TUI on caseio
├── src/lib.rs               # re-exports caseio + matrix builders
├── src/main.rs              # clap CLI: tui/batch/gen/verify/dcopf/sensitivities/convert
├── src/matrix/
│   ├── mod.rs               # BuildOptions, Scheme, MatrixStats, sddm_check
│   ├── triplet.rs           # CooBuilder (HashMap, O(nnz); new_rect for rectangular)
│   ├── bprime.rs / bdoubleprime.rs / ybus.rs / lacpf.rs / adjacency.rs
│   ├── incidence.rs         # A, b, B Aᵀ, P_shift; DcConvention
│   ├── laplacian.rs         # L = A diag(w) Aᵀ, ground_at, GroundMap, e_r
│   ├── sensitivity.rs       # PTDF, LODF (self-contained dense Cholesky)
│   └── opf.rs               # OpfInstance: Q, c, bounds, f̄, C_g, p_d; Units
├── src/io/                  # mtx (lower-triangle symmetric), npy, meta
├── src/pipeline.rs          # case → square MatrixKind family
├── src/opf_pipeline.rs      # case → DC-OPF bundle directory + manifest
├── src/synth/               # tree, lattice, pegase-like generators
└── src/tui/                 # ratatui app (in lib so it's testable)

casemat-ext/src/lib.rs       # PyO3 extension → COO triplets
python/casemat/              # importable package (scipy/networkx assembly)
python/tests/test_bindings.py
tests/data/                  # shared fixtures (used by CLI examples)
benchmarks/                  # parse benchmarks + Julia validation harnesses
```

## Things to know before editing

- **Workspace split.** `casemat` depends on `caseio` and re-exports it, so the
  matrix modules' `crate::case` / `crate::Error` / `crate::parser` paths resolve
  unchanged and a single `use casemat::...` pulls in both layers. Keep the
  parser/converter in `caseio` (light deps) and matrices in `casemat`.
- **Python packaging is two halves (maturin mixed layout).** `casemat-ext/` is
  the Rust PyO3 crate; it compiles to one native module, `casemat._casemat`
  (`[lib] name = _casemat`, `crate-type = cdylib`). `python/casemat/` is the
  pure-Python wrapper (`python-source = python` in pyproject) that turns the
  extension's COO triplets into `scipy.sparse`/networkx, keeping scipy out of
  the Rust build. `maturin develop` builds the crate and drops the `.so` into
  `python/casemat/`. One binding only: the `casemat` package surfaces caseio's
  parse/convert plus the matrices — there is no separate `caseio` Python package.
- **Lossless round-trip.** The MATPOWER parse retains the original source text
  and the writer echoes it, so `parse → write → parse` is byte-for-byte —
  every `mpc.*` field, in-matrix comments, and exact tokens like `7e-05`. Don't
  reformat through `f64` round-trips; don't drop fields the typed model ignores.
- **Two-tier fidelity contract.** Same-format round-trip is byte-exact.
  Cross-format conversion keeps maximal fidelity and reports anything the target
  can't represent in `Conversion::warnings` — never drop it silently.
- **Adding a format.** A reader and/or writer in `caseio/src/format/<name>.rs`
  that produces/consumes `Network`; register in `format/mod.rs`, re-export from
  `caseio/src/lib.rs`, add a CLI/`TargetFormat` arm. `Network` is the unifying
  hub; `MpcCase` is the MATPOWER-faithful typed model.
- **Sign convention.** Positive Laplacian: off diag negative, diag positive, `diag = sum |off-diag|` for B'. The positive (M-matrix) Laplacian form SDDM solvers expect.
- **Bus IDs.** MATPOWER 1 based; `MpcCase::bus_index(id)` is the only mapping into dense `[0, n)`. Don't clamp out of range — return `Error::UnknownBus`.
- **`BR_B` is already per unit.** Never divide by `base_mva` again.
- **`tap == 0` ⇒ `tap = 1`.** Use `Branch::effective_tap()`.
- **B' ignores taps and shifts. B'' zeros only shifts. Y_bus keeps both.**
- **DC-OPF Laplacian.** `L = A diag(b) Aᵀ` is built from the same `A`, `b` factors `build_incidence` returns (so `L` and the reweighted `L₁` share a factorization), and equals `build_bprime` in the XB scheme. Default `b = 1/x` (paper-pure); `DcConvention::Matpower` uses `1/(x·τ)` plus a phase-shift injection.
- **DC-OPF is bus-indexed.** Generation is nodal (`p_g ∈ ℝⁿ`), so `Q`, `c`, and bounds are length-n (zero at load buses), scattered from generator space through `C_g`; gen-space vectors ride along as provenance. Cost map: MATPOWER `c2 p² + c1 p` → `q = 2c2`, `c = c1`. Per-unit by default (`Units::PerUnit` scales `q` by `base²`, `c` by `base`).
- **`gen`/`gencost` are optional.** A power-flow-only case parses with `gens` empty; the OPF builders return `Error::NoGenerators`. Exactly one `BusType::Ref` is required (`MpcCase::reference_bus_index`).
- **PTDF/LODF need a solve.** They factor the slack-grounded Laplacian `ground_at(L, r)` (SPD) with a self-contained dense Cholesky (`matrix::sensitivity`) — no external solver dep. PTDF is dense `m×n`; large-scale sparse PTDF is future work.
- **MTX output is lower triangle, 1 based, spec compliant.** `sprs::io::write_matrix_market_sym` writes the *upper* triangle, so `io::mtx::write_mtx` ships its own writer.
- **`CooBuilder`.** HashMap COO with O(nnz) inserts; replaces the old O(nnz²) Vec search.
- **TUI lives in the library.** `casemat/src/tui/`, behind the `cli` feature. Testable via `ratatui::backend::TestBackend`. Binary calls `casemat::tui::run`.
- **petgraph view.** `MpcCase::to_petgraph()` returns `UnGraph<usize, usize>` where node weight = dense bus index, edge weight = branch index. Use it for connectivity, radial detection, spanning trees (LinDist3Flow).
- **`kkt` feature is experimental and local-only.** `src/matrix/kkt.rs` (repo root) is gitignored; the DC-OPF interior point operators behind `--features kkt` are not part of the default build.
- **Format validation needs Julia.** `benchmarks/validate_powermodels.jl` and `validate_psse.jl` check the writers/reader against PowerModels.jl; they don't run in plain `cargo test` (the all-pairs `caseio/tests/roundtrip_formats.rs` does).

## Test fixtures

`tests/data/case{9,14,30,57,118}.m` and `case2869pegase.m` are vendored verbatim
from `https://github.com/MATPOWER/matpower/tree/master/data` (BSD-3). Also
`t_case9_dcline.m`, `pglib/` (PGLib OPF), and `psse/*.raw` (PSS/E fixtures). Add
new sizes by curl from upstream.

## Relationship to GridFM

Intended as the fast Rust data layer beneath `gridfm-datakit` (Python, scenario generation) and `gridfm-graphkit` (PyTorch Geometric, GNN training). Planned Parquet output (issue #4) matches gridfm-datakit's column schemas.
