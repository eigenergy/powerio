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
- **`powerio-prob`**: problem instances (DC OPF, AC OPF, SCOPF) on `powerio`.
  Matrix free by default; `--features matrix` adds the sparse operators and
  the DC OPF bundle writer (`matrix::bundle::write_dcopf_bundle`).
- **`powerio-dist`**: the multiconductor distribution model (`DistNetwork`)
  with OpenDSS `.dss`, PMD JSON, and BMOPF JSON converters. Deliberately does
  **not** depend on `powerio`.
- **`powerio-pkg`**: the `.pio.json` document model (envelope, provenance,
  diagnostics, validation, operating points, study blocks, lowering). Depends
  on `powerio` and `powerio-dist`.
- **`powerio-cli`**: the `powerio` binary: the clap CLI and the ratatui TUI
  over `powerio-matrix`, `powerio-prob`, `powerio-dist`, and `powerio-pkg`.
- **`powerio-py`**: PyO3 extension behind the `powerio` Python package
  (`python/powerio/`); hands back COO triplets that scipy assembles.
- **`powerio-capi`**: C ABI over `powerio` (`pio_*`, header `powerio.h`) for
  C, C++, Julia, and other FFI users. Default features `dist,pkg` ship the
  `pio_dist_*` and `pio_package_*` surfaces. `--features arrow` adds
  `pio_to_arrow`, an Arrow C Data Interface export; `--features gridfm` adds
  `pio_read_dir` / `pio_scenario_ids` (the gridfm-datakit Parquet
  reader, pulling in `powerio-matrix`); `--features prob` adds `pio_scopf_*`
  and `pio_acopf_*`.
  All are additive and feature gated, so no ABI bump. Matrix Arrow ABI v1 is
  COO tables plus append only axis maps
  `matrix_bus` and `matrix_branch`.

`Network` is the one canonical model (format neutral, loads/shunts first class);
`IndexedNetwork` is the dense indexed analysis view derived from it.

Formats. MATPOWER `.m`, PowerModels JSON, PSS/E `.raw` (v33/34/35),
PowerWorld `.aux`, PSLF `.epc`, egret JSON, pandapower JSON, PyPSA CSV folders,
and Surge JSON all read and write. GO Challenge 3 JSON and DeepMind OPFData
JSON are read only inputs; PowerWorld `.pwb` is a read only binary input with
no writer. PowerWorld `.pwd` display files use the display API. GridFM Parquet
datasets read and write through directory helpers. Bare `Network` model JSON
moves through `Network::to_json`/`from_json`; since 0.7 it is not an advertised
case format (`powerio-json` stays only as a hidden, warned CLI token and a C
ABI v4 alias until 1.0). Distribution formats (OpenDSS `.dss`, PMD JSON, BMOPF
JSON) meet at `powerio-dist`'s `DistNetwork` the same way.
Every balanced case format meets at `Network`, so a new format is one
reader/writer at the hub, not a pairwise converter.

Matrix outputs (powerio-matrix):
- MATPOWER FDPF `Bp` (`bprime`): `-Im(Y_bus)` after clearing bus shunts and
  line charging and setting tap magnitudes to one. XB clears resistance. Phase
  shifts remain, matching MATPOWER `makeB`.
- MATPOWER FDPF `Bpp` (`bdoubleprime`): `-Im(Y_bus)` after clearing phase
  shifts. BX clears resistance. Line charging, bus shunts, and tap magnitudes
  remain.
- `Re(Y_bus)`, `-Im(Y_bus)` (full).
- LACPF block `[[G, -B], [-B, -G]]` (linear AC power flow, flat start, 2n×2n, indefinite).
- Adjacency (`MatrixKind::Adjacency`); PTDF and LODF (`sensitivities` subcommand).
- DC OPF instance bundle (`dcopf` subcommand; `powerio-prob`'s
  `matrix::bundle::write_dcopf_bundle`, `--features matrix`): signed incidence `A` (n×m), branch susceptance `b`, DC OPF Laplacian `L = A diag(b) Aᵀ` and its reference-grounded form, flow map `B Aᵀ`, generator cost `Q`/`c`, bounds, thermal limits `f̄`, generator→bus `C_g`, nodal load `p_d`, `e_r`.
- petgraph `UnGraph<bus_idx, branch_idx>` view + connectivity / radial diagnostics.
- gridfm-datakit Parquet dataset (`gridfm` subcommand, `io::gridfm::write_gridfm_dataset`, `--features gridfm`): the `bus_data`/`gen_data`/`branch_data`/`y_bus_data` tables a single parsed case maps to, matching gridfm-datakit's column schema so gridfm-graphkit trains on it directly.
- gridfm dataset → `Network` reader, the ML→classical return leg (`io::gridfm::read_gridfm_dataset` / `read_gridfm_scenarios` / `gridfm_base_case`, pure inverse `read_gridfm_network`; `--features gridfm`, issue #60). Lossy but complete for power flow: recovers bus types/voltages/limits, nodal load & shunt totals, generator dispatch & bounds (`vg` from bus `Vm`), branch `r/x/b/tap/shift/rate_a/angle-limits`, and `base_mva`; can't recover original bus ids (synthesized `1..n`), per element granularity (folded to one synthetic `Load`/`Shunt` per bus), piecewise/cubic costs, or HVDC/storage. Branches with unit effective tap and zero shift read back as lines (raw `tap 0`). Returns `GridfmRead { network, scenario, warnings }`; sets `SourceFormat::Gridfm`. One reader ⇒ gridfm → any classical writer for free. CLI: `convert <dataset-dir> --from gridfm [--scenario N] --to <fmt>` (kept out of the `parse_file` hub that has no parquet dependency). `y_bus_data` is ignored on read (branches carry raw `r/x/b`). Python: `read_gridfm(dir, scenario=0)` / `read_gridfm_scenarios(dir)` → `GridfmRead(network, scenario, warnings)`.

## Commands

```
cargo build --release        # the six default-members (all crates except powerio-py and powerio-capi)
cargo test                   # same six: powerio, -matrix, -prob, -cli, -dist, -pkg
cargo test -p powerio-capi   # the C ABI tests (not in default-members)
bash scripts/ci-clippy.sh    # full CI clippy matrix; run before pushing Rust/C ABI changes
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

## Release flow

PowerIO releases are tag driven.

1. Wait for `main` CI to pass on the merge commit that should become the
   release.
2. Check that the tag does not already exist, then create an annotated tag on
   `origin/main` and push it:

   ```
   git fetch origin main --tags
   git ls-remote --tags origin refs/tags/vX.Y.Z
   git tag -a vX.Y.Z origin/main -m vX.Y.Z
   git push origin vX.Y.Z
   ```

3. `.github/workflows/release-binaries.yml` runs on tag pushes. It builds the
   `powerio-capi` release tarballs for `aarch64-apple-darwin`,
   `aarch64-linux-gnu`, `x86_64-apple-darwin`, `x86_64-linux-gnu`, and
   `x86_64-w64-mingw32`, with the release features
   `arrow,matrix,gridfm,dist,pkg,prob`.
4. That workflow creates or updates a **draft** GitHub release and attaches the
   five binary assets. Do not expect a draft release to exist before the tag
   workflow runs.
5. A human inspects and publishes the draft release.
6. Publishing the release triggers `.github/workflows/notify-powerio-jl.yml`.
   If `POWERIO_JL_DISPATCH_TOKEN` is configured, it sends a
   `powerio-release` repository dispatch to `eigenergy/PowerIO.jl`. If the
   token is absent, the PowerIO.jl daily schedule or manual dispatch is the
   fallback.
7. PowerIO.jl's `.github/workflows/update-artifacts.yml` runs
   `julia gen/update_artifacts.jl <tag>`, checks the ABI handshake and the
   schema-version report, and tests the regenerated artifact. On the happy
   path it commits one atomic release commit to PowerIO.jl `main` and
   dispatches the registration workflow, with no human step. Prereleases,
   downgrades, and gate failures take a PR or park instead. The workflow
   stands down while any `artifacts/*` PR is open. Manual `update_artifacts`
   commands are a fallback, not the normal path.

## Layout

```
powerio/                      # parser + Network hub + converters
├── src/lib.rs               # public re-exports
├── src/error.rs             # thiserror Error + ErrorCategory
├── src/network.rs           # Network, Bus, Load, Shunt, Branch, Generator,
│                            #   GenCost, Storage, Hvdc, BusType, SourceFormat;
│                            #   to_json / from_json (the structured transport)
├── src/indexed.rs           # IndexCore, IndexedNetwork (dense indexed analysis
│                            #   view), ConnectivityReport; petgraph view:
│                            #   to_petgraph, is_radial, connectivity_report
├── src/normalize.rs         # Network::to_normalized (per unit/radian/filtered/
│                            #   reindexed derived view); shared per unit scaling
│                            #   (cost_to_pu/cost_from_pu, DEG_TO_RAD, GEN_PU_KEYS)
├── src/dc.rs                # DcConvention (shared DC susceptance convention)
├── src/gen_cost.rs          # GenCost model + quadratic projections
├── src/geo/                 # GeoLayer sidecar (layer.rs), .pwd harvest (pwd.rs)
├── src/operations.rs        # in place Network edit operations
├── src/solver_tables.rs     # Solver*Row tables + NormalizedSolverTables
├── src/format/
│   ├── mod.rs               # hub: parse_file, parse_str, convert_file, write_as,
│   │                        #   TargetFormat, Conversion, target_format_from_name
│   ├── routing.rs           # classify_json_text (bare .json routing)
│   ├── matpower/            # tokens, matlab, locate, rows, writer
│   │                        #   (the lossless source retaining path)
│   ├── powermodels.rs       # PowerModels JSON reader + writer
│   ├── goc3.rs              # GO Challenge 3 JSON reader
│   ├── opfdata.rs           # DeepMind OPFData JSON reader
│   ├── surge.rs             # Surge JSON reader + writer
│   ├── pandapower.rs        # pandapower JSON reader + writer
│   ├── pypsa.rs             # PyPSA CSV folder reader + writer
│   ├── pslf.rs              # PSLF EPC reader + writer
│   ├── psse.rs              # PSS/E .raw reader + writer
│   ├── powerworld/          # .aux reader + writer, .pwb reader, .pwd display
│   └── egret.rs             # egret JSON reader + writer
└── tests/                   # convert, roundtrip, roundtrip_formats, ...

powerio-matrix/               # matrices + graph views on powerio
├── src/lib.rs               # re-exports powerio + matrix builders
├── src/matrix/
│   ├── mod.rs               # BuildOptions, Scheme, MatrixStats, sddm_check
│   ├── triplet.rs           # CooBuilder (HashMap, O(nnz); new_rect for rectangular)
│   ├── bprime.rs / bdoubleprime.rs / ybus.rs / lacpf.rs / adjacency.rs
│   ├── incidence.rs         # A, b, B Aᵀ, P_shift
│   ├── laplacian.rs         # L = A diag(w) Aᵀ, ground_at, GroundedIndexMap, e_r
│   └── sensitivity.rs       # PTDF, LODF; SensitivityOptions (auto/dense/iterative)
├── src/io/                  # mtx (lower-triangle symmetric), meta, sensitivity,
│                            #   gridfm (gridfm-datakit Parquet, feature = "gridfm")
├── src/pipeline.rs          # case → square MatrixKind family
└── src/synth/               # tree, lattice, pegase-like generators

powerio-prob/                 # problem instances on powerio
├── src/dc.rs                # DcOpfInstance, build_dc_opf_instance
├── src/ac.rs                # AcOpfInstance, build_ac_opf_instance
├── src/scopf/               # ScopfInstance, GOC3 projection, versioned wire
└── src/matrix/bundle.rs     # DC OPF bundle directory + manifest (feature = "matrix")

powerio-dist/                 # multiconductor distribution model (no powerio dep)
├── src/model.rs             # DistNetwork + element tables
├── src/dss/ pmd/ bmopf/     # per format readers/writers
├── src/convert.rs           # hub: parse/convert + structured diagnostics
└── src/{graph,geo,diagnostics,error}.rs

powerio-pkg/                  # .pio.json compiler package envelope
├── src/package.rs           # NetworkPackage, schema version, materialization
├── src/operating.rs         # replayable operating point overlays
├── src/lowering.rs          # multiconductor → balanced lowering
└── src/{model,provenance,diagnostics,validation,study,summary,geo}.rs

powerio-cli/                  # the `powerio` binary (CLI + TUI)
├── src/main.rs              # clap CLI: tui/batch/gen/verify/dcopf/sensitivities/
│                            #   summary/package/gridfm/convert/geo
├── src/cases.rs             # recursive case discovery
└── src/tui/                 # ratatui app (app.rs, screens.rs, log_pane.rs, sparsity.rs, theme.rs)

powerio-py/src/lib.rs        # PyO3 extension → COO triplets (module `_powerio`)
python/powerio/              # importable package (scipy/networkx assembly, lazy)
python/tests/                # test_powerio.py, test_dist.py, test_geo.py,
                             #   test_gridfm.py, test_mcp.py, test_package.py
powerio-capi/                # C ABI (pio_*, include/powerio.h, examples/smoke.c)
│                            #   src/arrow_export.rs: pio_to_arrow (feature = "arrow")
tests/data/                  # shared fixtures (used by CLI examples)
benchmarks/                  # parse benchmarks + Julia validation harnesses
fuzz/                        # libFuzzer targets (detached workspace; see fuzz/README.md)
```

## Things to know before editing

- **Workspace split.** `powerio-matrix` depends on `powerio` and re-exports it,
  so the matrix modules' `crate::network` / `crate::Error` / `crate::format`
  paths resolve unchanged and a single `use powerio_matrix::...` pulls in both
  layers. Keep the parser/converter in `powerio` (light deps) and matrices in
  `powerio-matrix`.
- **Clippy must match CI.** Root `cargo clippy --all-targets` skips feature
  combinations and the PyO3 extension, so it misses failures that CI catches.
  Run `bash scripts/ci-clippy.sh` before pushing Rust, C ABI, Arrow, matrix,
  feature, or Python extension changes. Use a target such as
  `bash scripts/ci-clippy.sh capi-release` only while iterating on one failure.
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
- **Two-tier fidelity rules.** Same format round trip is byte exact.
  Cross-format conversion keeps maximal fidelity and reports anything the target
  can't represent in `Conversion::warnings`; never drop it silently.
- **Adding a format.** A reader and/or writer in `powerio/src/format/<name>.rs`
  that produces/consumes `Network`; register in `format/mod.rs`, re-export from
  `powerio/src/lib.rs`, add a CLI/`TargetFormat` arm. `Network` is the unifying
  hub.
- **JSON transport.** `Network::to_json`/`from_json` (serde) is the structured
  transport; over the C ABI it is `pio_to_json`/`pio_from_json`. The
  `powerio-json` format token was demoted from the case-format surface in 0.7:
  the CLI hides it behind a deprecation warning, and the C tokens stay only as
  ABI v4 aliases until 1.0. The retained
  `source` text is `#[serde(skip)]`, so JSON carries the tables, not the
  byte exact echo, and a `from_json` round trip returns `source` as `None`.
- **Distribution bindings stay lazy.** `pio_dist_parse_file` and
  `pio_dist_parse_str` return a live multiconductor handle. Julia display and
  scalar access use `pio_dist_summary_json`; `pio_dist_to_json` is the full
  element payload and should only run when a caller asks for `net.data` or an
  element table.
- **`.pio.json` packages.** `NetworkPackage` wraps one balanced or
  multiconductor payload with provenance, source maps, diagnostics, validation,
  lowering history, optional derived metadata, and optional `operating_points`.
  GOC3 package construction stores the static first interval in `model` and the
  source time series in replayable operating points. Materializing a point
  returns a derived static package with the series cleared.
- **Sign convention.** Positive Laplacians use negative off diag entries,
  positive diagonal entries, and `diag = sum |off-diag|`. This is the
  M-matrix form SDDM solvers expect.
- **Bus IDs.** MATPOWER 1 based; `IndexedNetwork::bus_index(id)` is the only mapping into dense `[0, n)`. Don't clamp out of range; return `Error::UnknownBus`.
- **`BR_B` is already per unit.** Never divide by `base_mva` again.
- **`tap == 0` ⇒ `tap = 1`.** Use `Branch::effective_tap()`.
- **MATPOWER FDPF matrices.** `build_bprime` follows MATPOWER `makeB` `Bp`:
  it approximates the active power versus voltage angle Jacobian block, clears
  bus shunts and line charging, sets tap magnitudes to one, clears resistance
  in the XB scheme, and keeps phase shifts. `build_bdoubleprime` follows
  MATPOWER `Bpp`: it approximates the reactive power versus voltage magnitude
  Jacobian block, clears phase shifts, clears resistance in the BX scheme, and
  keeps shunts, line charging, and tap magnitudes. `Y_bus` keeps taps and
  shifts.
- **Angle bound clamp postcondition.** When editing `clamp_angle_bounds`, test
  intervals wholly below `-pi/2` and wholly above `pi/2`; normalized branches
  must leave `angmin <= angmax`. Wide symmetric bounds and `0/0` already have
  coverage.
- **DC OPF Laplacian.** `L = A diag(b) Aᵀ` is built from the same `A`, `b`
  factors `build_incidence` returns. `DcConvention::series_susceptance` states
  the series susceptance `b`, negative for an inductive branch, the sign
  PowerModels `calc_branch_y` gives; `build_incidence` negates it once, because
  `L` takes the M-matrix form with negative off diagonals and positive
  diagonals. Never negate twice: the matrix comes out sign flipped and every
  builder downstream inherits it. Default `SeriesImpedance`: `b = -x/(r² + x²)`,
  which reads the whole series impedance, plus a phase shift injection, with no
  tap scaling. `Matpower` uses `-1/(x·τ)` plus the injection. `PaperPure`
  (`b = -1/x`, taps and shifts ignored) is deprecated in 0.9.0 and removed in
  1.0.0; with zero phase shifts it equals MATPOWER `Bp` in the XB scheme.
- **DC OPF lives in `powerio-prob`.** `DcOpfInstance` keeps generator-space
  data (`generators: DcGeneratorData`); `nodal_generator_data()` scatters it to
  bus space through `C_g` for length-n `Q`, `c`, bounds, and `has_gen`. Cost map: MATPOWER `c2 p² + c1 p` → `q = 2c2`, `c = c1`, constant `c0` retained. Per-unit by default (`Units::PerUnit` scales `q` by `base²`, `c` by `base`).
- **A bus can host several generators.** `nodal_generator_data()` (DC and AC) sums the bounds, which is exact, and combines the cost curves by the parallel rule `q = 1/Σ(1/qᵢ)` in `powerio-prob/src/nodal.rs`, which is the curve of the least cost split and therefore an approximation. One generator at a bus passes through unchanged.
- **`rate_a == 0` means unlimited.** `f_max`/`s_max` keep the zero. The opt-in `synthesize_unrated_limits` build option replaces it with `Branch::synthesize_rate_a`: the widest voltage phasor difference the terminal ceilings and the branch angle window allow, over `|Z|`, times the larger ceiling. The caller passes the window in radians and the method holds it at π; `angmin`/`angmax` are degrees in the neutral model and radians after `to_normalized`, so the builders convert them through `IndexedNetwork::angle_radians` and a raw case and its normalized form get the same bound.
- **`gen`/`gencost` are optional.** A power flow case with no `mpc.gen` parses with `gens` empty; the OPF builders return `Error::NoGenerators`.
- **Reference (slack) buses are a set, grounded one row/column each.** `IndexedNetwork::reference_bus_indices` returns every `BusType::Ref`; the matrix builders ground the whole set, so a network needs one reference *per connected component* (`IndexedNetwork::check_reference_coverage`). Several within one island is a distributed-slack solve. `reference_bus_index` is the exactly-one convenience query (errors otherwise) for the single-slack C/Python/gridfm paths.
- **PTDF/LODF need a solve.** They factor the reference grounded Laplacian (SPD when every island has a reference) in `matrix::sensitivity`; no external solver dep. The option based builders select dense Cholesky below the reduced-dimension threshold and a preconditioned conjugate gradient above it (`SensitivityOptions`, default `auto`). PTDF is dense `m×n`; sparse work would compute selected columns or use sparse factors, not make PTDF itself sparse.
- **MTX output is lower triangle, 1 based, spec compliant.** `sprs::io::write_matrix_market_sym` writes the *upper* triangle, so `io::mtx::write_mtx` ships its own writer.
- **`CooBuilder`.** HashMap COO with O(nnz) inserts; replaces the old O(nnz²) Vec search.
- **TUI lives in the CLI crate.** `powerio-cli/src/tui/`, part of the `powerio` binary. Testable via `ratatui::backend::TestBackend`.
- **petgraph view.** `IndexedNetwork::to_petgraph()` returns `UnGraph<usize, usize>` where node weight = dense bus index, edge weight = branch index. Use it for connectivity and radial detection.
- **Format validation needs Julia.** `benchmarks/validate_powermodels.jl` and `validate_psse.jl` check the writers/reader against PowerModels.jl; they don't run in plain `cargo test` (the all-pairs `powerio/tests/roundtrip_formats.rs` does).

## Test fixtures

`tests/data/case{9,14,30,57,118}.m` and `case2869pegase.m` are vendored verbatim
from `https://github.com/MATPOWER/matpower/tree/master/data` (BSD-3). Also
`t_case9_dcline.m`, `pglib/` (PGLib OPF), and `psse/*.raw` (PSS/E fixtures).
`tests/data/pandapower/example.json` was written by pandapower 3.2.2 and
`tests/data/pypsa/example/` by PyPSA 1.2.2; both are committed byte exact as
the tool wrote them (generation snippets in the READMEs next to them).

Use the smallest fixture that exercises the behavior under test. Never add a
fixture larger than 100 KiB unless the user explicitly approves that exact file
after being told its byte count, line count, source, license, and effect on the
pull request. Approval of a broader implementation plan does not authorize
vendoring its test data. Do not commit a fixture without a license that permits
redistribution. Prefer synthetic fixtures unless byte exact source fidelity is
the behavior under test. Run `wc -lc` and inspect `git diff --stat` before
committing any new fixture.

## Relationship to GridFM

Intended as the fast Rust data layer beneath `gridfm-datakit` (Python, scenario generation) and `gridfm-graphkit` (PyTorch Geometric, GNN training). The `gridfm` subcommand (`io::gridfm`, `--features gridfm`, issue #4) writes the `bus_data`/`gen_data`/`branch_data`/`y_bus_data` Parquet tables matching gridfm-datakit's column schema, under `<out>/<case>/raw/`, so gridfm-graphkit's `HeteroGridDatasetDisk` loads powerio output directly. powerio has no solver, so a case is one snapshot (`scenario 0`): voltages/dispatch are the case's stored values and branch flows are computed from them. A scenario batch (`write_gridfm_batch` / `GridfmSnapshot`, or multiple `gridfm` CLI inputs) row-stacks snapshots that share one base element set, keyed by the `scenario` column.
