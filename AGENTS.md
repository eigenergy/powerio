# AGENTS.md

Guidance for agents working in this repo.

## Purpose

A Cargo workspace of Rust crates plus a Python package. Parses power network
case files, converts losslessly between formats, and emits sparse matrices and
graph views for any downstream solver. Feeds the GridFM ML pipeline.

- **`powerio`**: the parser, the format-neutral `Network` hub, the lossless
  writer, and the format converters. Light deps (thiserror, num-complex,
  petgraph, serde, serde_json, lexical-core); no matrix or TUI stack.
- **`powerio-matrix`**: sparse matrices and graph views built on `powerio`
  (which it re-exports).
- **`powerio-cli`**: the `powerio` binary: the clap CLI and the ratatui TUI
  over `powerio-matrix`.
- **`powerio-py`**: PyO3 extension behind the `powerio` Python package
  (`python/powerio/`); hands back COO triplets that scipy assembles.
- **`powerio-capi`**: C ABI over `powerio` (`pio_*`, header `powerio.h`) for
  C, C++, Julia, and other FFI users. `--features arrow` adds
  `pio_to_arrow`, an Arrow C Data Interface export; `--features gridfm` adds
  `pio_read_dir` / `pio_scenario_ids` (the gridfm-datakit Parquet
  reader, pulling in `powerio-matrix`); `--features dist` adds `pio_dist_*`;
  `--features pkg` adds `pio_package_*`. All are additive and feature gated, so
  no ABI bump.

`Network` is the one canonical model (format neutral, loads/shunts first class);
`IndexedNetwork` is the dense indexed analysis view derived from it.

Formats. MATPOWER `.m`, PowerModels JSON, PSS/E `.raw` (v33/34/35),
PowerWorld `.aux`, PSLF `.epc`, egret JSON, pandapower JSON, PyPSA CSV folders,
Surge JSON, and PowerIO JSON all read and write. GO Challenge 3 JSON is a read
only input with byte exact same source echo; PowerWorld `.pwb` is a read only
binary input with no writer. PowerWorld `.pwd` display files use the display
API. GridFM Parquet datasets read and write through directory helpers.
Every balanced case format meets at `Network`, so a new format is one
reader/writer at the hub, not a pairwise converter.

Matrix outputs (powerio-matrix):
- B' (FDPF, shuntless). Singular positive Laplacian, rank n-1.
- B'' (FDPF, with shunts and taps). SDDM when bus shunts are present.
- `Re(Y_bus)`, `-Im(Y_bus)` (full).
- LACPF block `[[G, -B], [-B, -G]]` (linear AC power flow, flat start, 2n×2n, indefinite).
- Adjacency (`MatrixKind::Adjacency`); PTDF and LODF (`sensitivities` subcommand).
- DC OPF instance bundle (`dcopf` subcommand, `opf_pipeline::write_dcopf_bundle`): signed incidence `A` (n×m), branch susceptance `b`, weighted Laplacian `L = A diag(b) Aᵀ` and its reference-grounded form, flow map `B Aᵀ`, generator cost `Q`/`c`, bounds, thermal limits `f̄`, generator→bus `C_g`, nodal load `p_d`, `e_r`.
- petgraph `UnGraph<bus_idx, branch_idx>` view + connectivity / radial diagnostics.
- gridfm-datakit Parquet dataset (`gridfm` subcommand, `io::gridfm::write_gridfm_dataset`, `--features gridfm`): the `bus_data`/`gen_data`/`branch_data`/`y_bus_data` tables a single parsed case maps to, matching gridfm-datakit's column schema so gridfm-graphkit trains on it directly.
- gridfm dataset → `Network` reader, the ML→classical return leg (`io::gridfm::read_gridfm_dataset` / `read_gridfm_scenarios` / `gridfm_base_case`, pure inverse `read_gridfm_network`; `--features gridfm`, issue #60). Lossy but power-flow-complete: recovers bus types/voltages/limits, nodal load & shunt totals, generator dispatch & bounds (`vg` from bus `Vm`), branch `r/x/b/tap/shift/rate_a/angle-limits`, and `base_mva`; can't recover original bus ids (synthesized `1..n`), per-element granularity (folded to one synthetic `Load`/`Shunt` per bus), piecewise/cubic costs, or HVDC/storage. Unit effective-tap/zero-shift branches read back as lines (raw `tap 0`). Returns `GridfmRead { network, scenario, warnings }`; sets `SourceFormat::Gridfm`. One reader ⇒ gridfm → any classical writer for free. CLI: `convert <dataset-dir> --from gridfm [--scenario N] --to <fmt>` (stays out of the parquet-free `parse_file` hub). `y_bus_data` is ignored on read (branches carry raw `r/x/b`). Python: `read_gridfm(dir, scenario=0)` / `read_gridfm_scenarios(dir)` → `GridfmRead(network, scenario, warnings)`.

## Commands

```
cargo build --release        # powerio + powerio-matrix + powerio-cli (default-members)
cargo test                   # powerio + powerio-matrix (default-members)
cargo test -p powerio-capi   # the C ABI tests (not in default-members)
cargo clippy --all-targets
cargo fmt --all --check      # rustfmt is enforced (edition 2024)

# CLI (the binary is `powerio`):
powerio                                                   # TUI
powerio batch -i tests/data -o out --matrices bprime,bdoubleprime
powerio gen --topology lattice --n 1024 -o out
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio convert tests/data/case14.m --to psse -o case14.raw
powerio package tests/data/case14.m -o case14.pio.json
powerio gridfm tests/data/case14.m -o out      # gridfm-datakit Parquet dataset

# C ABI (cdylib + staticlib; header powerio-capi/include/powerio.h):
cargo build -p powerio-capi
cargo build -p powerio-capi --features arrow   # + pio_to_arrow (Arrow C Data Interface)

# Python (PyO3 crate needs libpython, so it is NOT in default-members):
cargo build -p powerio-py    # plain cargo build of the extension
maturin develop              # build + install the `powerio` wheel into the active venv
maturin develop -E all       # also pull scipy/numpy/networkx for the matrix + graph paths
pytest python/tests
```

## Layout

```
powerio/                      # parser + Network hub + converters
├── src/lib.rs               # public re-exports
├── src/error.rs             # thiserror Error
├── src/network.rs           # Network, Bus, Load, Shunt, Branch, Generator,
│                            #   GenCost, Storage, Hvdc, BusType, SourceFormat;
│                            #   to_json / from_json (the structured transport)
├── src/indexed.rs           # IndexCore, IndexedNetwork (dense indexed analysis
│                            #   view), ConnectivityReport; petgraph view:
│                            #   to_petgraph, is_radial, connectivity_report
├── src/normalize.rs         # Network::to_normalized (per unit/radian/filtered/
│                            #   reindexed derived view); shared per unit scaling
│                            #   (cost_to_pu/cost_from_pu, DEG_TO_RAD, GEN_PU_KEYS)
├── src/format/
│   ├── mod.rs               # hub: parse_file, parse_str, convert_file, write_as,
│   │                        #   TargetFormat, Conversion, target_format_from_name
│   ├── matpower/            # tokens, matlab, locate, rows, writer
│   │                        #   (the lossless source retaining path)
│   ├── powermodels.rs       # PowerModels JSON reader + writer
│   ├── goc3.rs              # GO Challenge 3 JSON reader
│   ├── surge.rs             # Surge JSON reader + writer
│   ├── pandapower.rs        # pandapower JSON reader + writer
│   ├── pypsa.rs             # PyPSA CSV folder reader + writer
│   ├── pslf.rs              # PSLF EPC reader + writer
│   ├── psse.rs              # PSS/E .raw reader + writer
│   ├── powerworld.rs        # PowerWorld .aux reader + writer
│   └── egret.rs             # egret JSON reader + writer
└── tests/                   # convert, roundtrip, roundtrip_formats

powerio-matrix/               # matrices + graph views on powerio
├── src/lib.rs               # re-exports powerio + matrix builders
├── src/matrix/
│   ├── mod.rs               # BuildOptions, Scheme, MatrixStats, sddm_check
│   ├── triplet.rs           # CooBuilder (HashMap, O(nnz); new_rect for rectangular)
│   ├── bprime.rs / bdoubleprime.rs / ybus.rs / lacpf.rs / adjacency.rs
│   ├── incidence.rs         # A, b, B Aᵀ, P_shift; DcConvention
│   ├── laplacian.rs         # L = A diag(w) Aᵀ, ground_at, GroundedIndexMap, e_r
│   ├── sensitivity.rs       # PTDF, LODF (self contained dense Cholesky)
│   ├── opf.rs               # OpfInstance: Q, c, bounds, f̄, C_g, p_d; Units
│   └── kkt.rs               # DC OPF interior point operators (feature = "kkt")
├── src/io/                  # mtx (lower-triangle symmetric), meta,
│                            #   gridfm (gridfm-datakit Parquet, feature = "gridfm")
├── src/pipeline.rs          # case → square MatrixKind family
├── src/opf_pipeline.rs      # case → DC OPF bundle directory + manifest
└── src/synth/               # tree, lattice, pegase-like generators

powerio-cli/                  # the `powerio` binary (CLI + TUI)
├── src/main.rs              # clap CLI: tui/batch/gen/verify/dcopf/sensitivities/convert
└── src/tui/                 # ratatui app (app.rs, screens.rs, log_pane.rs, sparsity.rs, theme.rs)

powerio-pkg/                  # .pio.json compiler package envelope
├── src/package.rs           # NetworkPackage, schema version, materialization
├── src/operating.rs         # replayable operating point overlays
└── src/lowering.rs          # multiconductor → balanced lowering

powerio-py/src/lib.rs        # PyO3 extension → COO triplets (module `_powerio`)
python/powerio/              # importable package (scipy/networkx assembly, lazy)
python/tests/               # test_powerio.py, test_gridfm.py, test_mcp.py
powerio-capi/                # C ABI (pio_*, include/powerio.h, examples/smoke.c)
│                            #   src/arrow_export.rs: pio_to_arrow (feature = "arrow")
tests/data/                  # shared fixtures (used by CLI examples)
benchmarks/                  # parse benchmarks + Julia validation harnesses
```

## Things to know before editing

- **Workspace split.** `powerio-matrix` depends on `powerio` and re-exports it,
  so the matrix modules' `crate::network` / `crate::Error` / `crate::format`
  paths resolve unchanged and a single `use powerio_matrix::...` pulls in both
  layers. Keep the parser/converter in `powerio` (light deps) and matrices in
  `powerio-matrix`.
- **One Python wheel (maturin mixed layout).** `powerio-py/` is the Rust PyO3
  crate; it compiles to one native module, `powerio._powerio` (`[lib] name =
  _powerio`, `crate-type = cdylib`). `python/powerio/` is the pure-Python
  wrapper (`python-source = python` in the root pyproject) that turns the
  extension's COO triplets into `scipy.sparse`/networkx. No numpy at the Rust
  layer: the triplets cross as plain Python lists, so `import powerio` and
  parse/write/convert pull in nothing but the interpreter. scipy/numpy/networkx
  are optional extras (`powerio[matrix]`, `[graph]`, `[all]`); a missing one
  raises a clear ImportError. `maturin develop` drops the `.so` into
  `python/powerio/`. One package surfaces both halves: parse/convert and the
  matrices.
- **Lossless writeback.** The MATPOWER parse retains the original source text
  and the writer returns it, so `parse → write → parse` keeps the exact bytes:
  every `mpc.*` field, in-matrix comments, and exact tokens like `7e-05`. Don't
  reformat through `f64` round-trips; don't drop fields the typed model ignores.
- **Two-tier fidelity contract.** Same format round trip is byte exact.
  Cross-format conversion keeps maximal fidelity and reports anything the target
  can't represent in `Conversion::warnings`; never drop it silently.
- **Adding a format.** A reader and/or writer in `powerio/src/format/<name>.rs`
  that produces/consumes `Network`; register in `format/mod.rs`, re-export from
  `powerio/src/lib.rs`, add a CLI/`TargetFormat` arm. `Network` is the unifying
  hub.
- **JSON transport.** `Network::to_json`/`from_json` (serde) is the structured
  transport; over the C ABI it is the `powerio-json` format through `pio_to_format`/`pio_parse_str`. The retained
  `source` text is `#[serde(skip)]`, so JSON carries the tables, not the
  byte exact echo, and a `from_json` round trip returns `source` as `None`.
- **`.pio.json` packages.** `NetworkPackage` wraps one balanced or
  multiconductor payload with provenance, source maps, diagnostics, validation,
  lowering history, optional derived metadata, and optional `operating_points`.
  GOC3 package construction stores the static first interval in `model` and the
  source time series in replayable operating points. Materializing a point
  returns a derived static package with the series cleared.
- **Sign convention.** Positive Laplacian: off diag negative, diag positive, `diag = sum |off-diag|` for B'. The positive (M-matrix) Laplacian form SDDM solvers expect.
- **Bus IDs.** MATPOWER 1 based; `IndexedNetwork::bus_index(id)` is the only mapping into dense `[0, n)`. Don't clamp out of range; return `Error::UnknownBus`.
- **`BR_B` is already per unit.** Never divide by `base_mva` again.
- **`tap == 0` ⇒ `tap = 1`.** Use `Branch::effective_tap()`.
- **B' ignores taps and shifts. B'' zeros only shifts. Y_bus keeps both.**
- **DC OPF Laplacian.** `L = A diag(b) Aᵀ` is built from the same `A`, `b` factors `build_incidence` returns (so `L` and the reweighted `L₁` share a factorization), and equals `build_bprime` in the XB scheme. Default `b = 1/x` (paper-pure); `DcConvention::Matpower` uses `1/(x·τ)` plus a phase-shift injection.
- **DC OPF is bus indexed.** Generation is nodal (`p_g ∈ ℝⁿ`), so `Q`, `c`, and bounds are length n (zero at load buses), scattered from generator space through `C_g`; gen-space vectors (`OpfInstance::gen_costs`) ride along as provenance. Cost map: MATPOWER `c2 p² + c1 p` → `q = 2c2`, `c = c1`. Per-unit by default (`Units::PerUnit` scales `q` by `base²`, `c` by `base`).
- **`gen`/`gencost` are optional.** A power flow case with no `mpc.gen` parses with `gens` empty; the OPF builders return `Error::NoGenerators`.
- **Reference (slack) buses are a set, grounded one row/column each.** `IndexedNetwork::reference_bus_indices` returns every `BusType::Ref`; the matrix builders ground the whole set, so a network needs one reference *per connected component* (`IndexedNetwork::check_reference_coverage`). Several within one island is a distributed-slack solve. `reference_bus_index` is the exactly-one convenience query (errors otherwise) for the single-slack C/Python/gridfm paths.
- **PTDF/LODF need a solve.** They factor the reference grounded Laplacian (SPD when every island has a reference) with a self contained dense Cholesky (`matrix::sensitivity`); no external solver dep. PTDF is dense `m×n`; sparse work would compute selected columns or use sparse factors, not make PTDF itself sparse.
- **MTX output is lower triangle, 1 based, spec compliant.** `sprs::io::write_matrix_market_sym` writes the *upper* triangle, so `io::mtx::write_mtx` ships its own writer.
- **`CooBuilder`.** HashMap COO with O(nnz) inserts; replaces the old O(nnz²) Vec search.
- **TUI lives in the CLI crate.** `powerio-cli/src/tui/`, part of the `powerio` binary. Testable via `ratatui::backend::TestBackend`.
- **petgraph view.** `IndexedNetwork::to_petgraph()` returns `UnGraph<usize, usize>` where node weight = dense bus index, edge weight = branch index. Use it for connectivity and radial detection.
- **`kkt` feature is experimental and off by default.** `powerio-matrix/src/matrix/kkt.rs` holds the DC OPF interior point operators behind `--features kkt`; not part of the default build or the main CI jobs.
- **Format validation needs Julia.** `benchmarks/validate_powermodels.jl` and `validate_psse.jl` check the writers/reader against PowerModels.jl; they don't run in plain `cargo test` (the all-pairs `powerio/tests/roundtrip_formats.rs` does).

## Test fixtures

`tests/data/case{9,14,30,57,118}.m` and `case2869pegase.m` are vendored verbatim
from `https://github.com/MATPOWER/matpower/tree/master/data` (BSD-3). Also
`t_case9_dcline.m`, `pglib/` (PGLib OPF), and `psse/*.raw` (PSS/E fixtures). Add
new sizes by curl from upstream.
`tests/data/pandapower/example.json` was written by pandapower 3.2.2 and
`tests/data/pypsa/example/` by PyPSA 1.2.2; both are committed byte exact as
the tool wrote them (generation snippets in the READMEs next to them).

## Relationship to GridFM

Intended as the fast Rust data layer beneath `gridfm-datakit` (Python, scenario generation) and `gridfm-graphkit` (PyTorch Geometric, GNN training). The `gridfm` subcommand (`io::gridfm`, `--features gridfm`, issue #4) writes the `bus_data`/`gen_data`/`branch_data`/`y_bus_data` Parquet tables matching gridfm-datakit's column schema, under `<out>/<case>/raw/`, so gridfm-graphkit's `HeteroGridDatasetDisk` loads powerio output directly. powerio has no solver, so a case is one snapshot (`scenario 0`): voltages/dispatch are the case's stored values and branch flows are computed from them. A scenario batch (`write_gridfm_batch` / `GridfmSnapshot`, or multiple `gridfm` CLI inputs) row-stacks snapshots that share one base element set, keyed by the `scenario` column.
