# AGENTS.md

Guidance for agents working in this repo.

## Purpose

A Cargo workspace of Rust crates plus a Python package. Parses power network
sources, converts between formats, and emits sparse matrices and graph data for
downstream solvers. Feeds the GridFM ML pipeline.

The 1.0 public design is recorded under `arch-v1/` in `V1_TERMINOLOGY.md`,
`V1_ARCHITECTURE.md`, `V1_ONTOLOGY.md`, `V1_ISSUE_AUDIT.md`, and
`V1_IMPLEMENTATION.md`. `V1_RATIONALE.md` explains why the settled choices won
over the alternatives. Review them before changing a public name or meaning.

- **`powerio-core`**: the 1.0 shared foundation: `Source`, `FormatId`,
  `Diagnostic`, `Error`, `PioModule<T>`, common module records,
  `TimeSeries<T>`, `ScenarioSet<T>`, and output destination types. It has no
  electrical network, matrix, or solver dependencies. The 0.x `powerio-diag`
  and `powerio-pkg` packages retire at 1.0.
- **`powerio`**: currently the parser, format neutral `BalancedNetwork`, writer,
  and format converters. In 1.0 this implementation crate becomes
  `powerio-tx`, while the short `powerio` name becomes the entry facade.
- **`powerio-matrix`**: sparse matrices and graph data built on
  `powerio-tx`, `powerio-dist`, and `powerio-prob`. In 1.0 it no longer
  depends on or re-exports the top `powerio` facade, which would create a
  dependency cycle.
- **`powerio-prob`**: operating points, problem instances, and solutions on
  `powerio-tx` and `powerio-dist`.
  The 1.0 public names are
  defined in `arch-v1/V1_TERMINOLOGY.md`; the current ambiguous security constrained
  type must not remain public.
  It stays matrix free; sparse operators and the DC OPF bundle writer move to
  `powerio-matrix` so dependencies continue upward without a feature cycle.
- **`powerio-dist`**: the multiconductor distribution model (`MulticonductorNetwork`)
  with OpenDSS `.dss`, PMD JSON, and BMOPF JSON converters. Deliberately does
  **not** depend on the transmission network crate; it shares `powerio-core`.
  BMOPF network decoding lives here; construction of the
  resulting `McAcOpfInstance` lives in `powerio-prob`.
- **1.0 `powerio` facade**: owns `PioValue`, `PioValueKind`, universal format dispatch, the
  `.pio.json` schema and upgrade reader, and public re-exports. The current
  `powerio-pkg` crate dissolves; do not recreate its package shaped API.
- **`powerio-cli`**: the `powerio` binary: the clap CLI and the ratatui TUI
  over the `powerio` facade and its component crates.
- **`powerio-py`**: PyO3 extension behind the `powerio` Python package
  (`python/powerio/`); hands back COO triplets that scipy assembles.
- **`powerio-capi`**: C ABI over `powerio` (`pio_*`, header `powerio.h`) for C, C++, Julia, and other FFI users. The current ABI v5 feature and function names for `.pio.json` are replaced consistently in ABI v6. `--features arrow` adds `pio_to_arrow`, an Arrow C Data Interface export; `--features gridfm` adds `pio_read_dir` / `pio_scenario_ids` (the gridfm-datakit Parquet parser, pulling in `powerio-matrix`); `--features prob` adds the current problem instance functions, which ABI v6 replaces with the 1.0 names. Those v5 feature additions were additive. The 1.0 type and ownership changes ship together as ABI v6. The matrix Arrow tables are COO tables plus row and column mapping tables `matrix_bus` and `matrix_branch`, versioned append only with no separate number: a removed table's id is burned, never reused, and the Arrow catalog report is stamped with the package version.

`BalancedNetwork` and `MulticonductorNetwork` are the two reusable electrical
network types. `IndexedNetwork`, normalized tables, and dense row arrays are
internal compiler data in 1.0, not public ontology types.

Formats. MATPOWER `.m`, PowerModels JSON, PSS/E `.raw` (v33/34/35),
PowerWorld `.aux`, PSLF `.epc`, Egret JSON, pandapower JSON, PyPSA CSV directories,
and Surge JSON all parse and write. DOE GO Challenge 3 JSON and DeepMind OPFData
JSON are parse only inputs; PowerWorld `.pwb` is a parse only binary input with
no writer. PowerWorld `.pwd` display files use the display API. GridFM Parquet
directories parse and write through directory helpers. PowerIO network JSON
moves through `Network::to_json`/`from_json`; it is a network serialization
rather than a case format, so 0.9 removed the last `powerio-json` token from
every surface and a bare `.json` holding it classifies as `model-json`.
OpenDSS `.dss` and PMD engineering JSON meet at `powerio-dist`'s
`MulticonductorNetwork`. BMOPF JSON defines an optimization calculation and
produces `McAcOpfInstance`; its electrical decoding reuses `powerio-dist`.
Traditional balanced network formats map to `BalancedNetwork`, so a new format
needs one parser and writer rather than pairwise converters. PyPSA 1.0
support is the documented CSV electrical profile and produces
`BalancedNetwork`, `TimeSeries<BalancedNetwork>`, or, when only a complete
electrical state varies, `TimeSeries<OperatingPoint<BalancedNetwork>>`.
Snapshot-local electrical series in the profile are typed. Non-electrical
components, intertemporal calculation data, investment periods, and stochastic
data outside that profile are retained for exact same format writing and
diagnosed before cross-format projection. Full
PyPSA and NetCDF support waits for source neutral multi-carrier, multi-period,
capacity expansion, stochastic calculation, and result types.

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
- DC matrix data follows PowerModels: incidence `A` is branches by buses with
  `+1` at the from bus and `-1` at the to bus, `B = A' * Diagonal(b) * A`, and
  `Bf = Diagonal(b) * A`. Phase shift injection stays separate.
- petgraph `UnGraph<bus_idx, branch_idx>` data plus connectivity and radial
  diagnostics.
- GridFM Parquet directory writing through `gridfm-datakit` compatible tables.
- GridFM Parquet directory parsing reuses one balanced network identity set
  across scenarios in 1.0 rather than cloning one network per scenario.

## Commands

```
cargo build --release        # default workspace members except powerio-py and powerio-capi
cargo test                   # default workspace members
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
powerio gridfm tests/data/case14.m -o out      # GridFM Parquet directory

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
2. Merge the reviewed PowerIO.jl release intent for the same version before
   tagging PowerIO. PowerIO.jl's `Project.toml`, top `CHANGELOG.md` section,
   and `.github/powerio-release.toml` must agree on the Julia version and
   `vX.Y.Z` PowerIO tag. The intent is marked ready only after its canonical
   source digest matches the reviewed PowerIO.jl tree.
3. Check that the tag does not already exist, then create an annotated tag on
   `origin/main` and push it:

   ```
   git fetch origin main --tags
   git ls-remote --tags origin refs/tags/vX.Y.Z
   git tag -a vX.Y.Z origin/main -m vX.Y.Z
   git push origin vX.Y.Z
   ```

4. `.github/workflows/release-binaries.yml` runs on tag pushes. Its binding gate tests PowerIO.jl `main` against the tagged library before it builds the `powerio-capi` release tarballs for `aarch64-apple-darwin`, `aarch64-linux-gnu`, `x86_64-apple-darwin`, `x86_64-linux-gnu`, and `x86_64-w64-mingw32`, with the release features `arrow,matrix,gridfm,dist,prob`.
5. That workflow creates or updates a **draft** GitHub release and attaches the
   five binary assets. Do not expect a draft release to exist before the tag
   workflow runs.
6. A human inspects and publishes the draft release.
7. Publishing the release triggers `.github/workflows/notify-powerio-jl.yml`.
   If `POWERIO_JL_DISPATCH_TOKEN` is configured, it sends a
   `powerio-release` repository dispatch to `eigenergy/PowerIO.jl`. If the
   token is absent, the PowerIO.jl daily schedule or manual dispatch is the
   fallback.
8. PowerIO.jl's `.github/workflows/update-artifacts.yml` accepts only the tag
   named by the ready release intent. It verifies the published release and
   exact five assets, registry order, absence of an open `artifacts/*` PR, the
   ABI handshake, schema report, and full Julia tests. It may change only
   `Artifacts.toml`. After confirming that PowerIO.jl `main` is still the
   reviewed base SHA, it commits that one file to `main` and dispatches
   registration for the exact resulting SHA. A schedule is the backstop and a
   manual dispatch retries the same intent; neither can override its version,
   tag, changelog, or source digest.

## Layout

```
powerio-tx/                   # 1.0 name of the current balanced parser crate
├── src/lib.rs               # public re-exports
├── src/network.rs           # Network, Bus, Load, Shunt, Branch, Generator,
│                            #   GenCost, Storage, Hvdc, BusType, SourceFormat;
│                            #   to_json / from_json (the structured transport)
├── src/indexed.rs           # IndexCore, IndexedNetwork (dense indexed analysis
│                            #   data), ConnectivityReport; petgraph data:
│                            #   to_petgraph, is_radial, connectivity_report
├── src/normalize.rs         # Network::to_normalized (per unit/radian/filtered/
│                            #   reindexed derived view); shared per unit scaling
│                            #   (cost_to_pu/cost_from_pu, DEG_TO_RAD, GEN_PU_KEYS)
├── src/dc.rs                # current DC formula selection; rename for 1.0
├── src/gen_cost.rs          # GenCost model + quadratic projections
├── src/geo/                 # GeoLayer sidecar (layer.rs), .pwd harvest (pwd.rs)
├── src/operations.rs        # in place Network edit operations
├── src/solver_tables.rs     # current internal solver preparation data
├── src/format/
│   ├── mod.rs               # parse (Source based), convert_file, write_as,
│   │                        #   TargetFormat, Conversion, target_format_from_name
│   ├── routing.rs           # classify_json_text (bare .json routing)
│   ├── matpower/            # tokens, matlab, locate, rows, writer
│   │                        #   (the lossless source retaining path)
│   ├── powermodels.rs       # PowerModels JSON parser + writer
│   ├── surge.rs             # Surge JSON parser + writer
│   ├── pandapower.rs        # pandapower JSON parser + writer
│   ├── pslf.rs              # PSLF EPC parser + writer
│   ├── psse.rs              # PSS/E .raw parser + writer
│   ├── powerworld/          # .aux parser + writer, .pwb parser, .pwd display
│   └── egret.rs             # Egret JSON parser + writer
└── tests/                   # convert, roundtrip, roundtrip_formats, ...

powerio-matrix/               # matrices + graph data; 1.0 uses component crates
├── src/lib.rs               # current re-exports plus matrix builders
├── src/matrix/
│   ├── mod.rs               # BuildOptions, Scheme, MatrixStats, sddm_check
│   ├── triplet.rs           # CooBuilder (HashMap, O(nnz); new_rect for rectangular)
│   ├── bprime.rs / bdoubleprime.rs / ybus.rs / lacpf.rs / adjacency.rs
│   ├── incidence.rs         # A, b, B, Bf, phase shift injection
│   ├── laplacian.rs         # current internal factor matrix utilities
│   └── sensitivity.rs       # PTDF, LODF; SensitivityOptions (auto/dense/iterative)
├── src/io/                  # mtx (lower-triangle symmetric), meta, sensitivity,
│                            #   gridfm (gridfm-datakit Parquet, feature = "gridfm")
├── src/bundle/              # DC OPF bundle directory + manifest
├── src/pipeline.rs          # source → supported square MatrixKind values
└── src/synth/               # tree, lattice, pegase-like generators

powerio-prob/                 # operating points, problem instances, solutions
├── src/operating.rs         # OperatingPoint and type specific series builders
├── src/dc.rs                # DC power flow and OPF instances and solutions
├── src/ac.rs                # AC balanced and multiconductor instances and solutions
└── src/format/              # DOE GO Challenge 3, DeepMind OPFData, BMOPF assembly

powerio-core/                 # dependency neutral shared foundation
├── src/module.rs            # PioModule<T> and common records
└── src/{source,diagnostic,error,time_series,scenario,output}.rs

powerio-dist/                 # multiconductor distribution model (no powerio dep)
├── src/model.rs             # MulticonductorNetwork + element tables
├── src/dss/ pmd/ bmopf/     # network parsers, writers, and BMOPF electrical decoder
├── src/convert.rs           # parse/convert + structured diagnostics
└── src/{graph,geo,diagnostics,error}.rs

powerio/                      # 1.0 facade, PioValue, dispatch, .pio.json, re-exports
├── src/value.rs             # PioValue, PioValueKind, typed narrowing
├── src/dispatch.rs          # source classification and component parser routing
└── src/stored/              # exact .pio.json DTOs and one way upgrade reader

powerio-cli/                  # the `powerio` binary (CLI + TUI)
├── src/main.rs              # clap CLI: tui/batch/gen/verify/dcopf/sensitivities/
│                            #   current summary and .pio.json commands, gridfm/convert/geo
├── src/cases.rs             # recursive case discovery
└── src/tui/                 # ratatui app (app.rs, screens.rs, log_pane.rs, sparsity.rs, theme.rs)

powerio-py/src/lib.rs        # PyO3 extension → COO triplets (module `_powerio`)
python/powerio/              # importable package (scipy/networkx assembly, lazy)
python/tests/                # test_powerio.py, test_dist.py, test_geo.py,
                             #   test_gridfm.py, test_mcp.py, test_package.py
powerio-capi/                # C ABI (pio_*, include/powerio.h, examples/smoke.c)
│                            #   src/arrow_export.rs: pio_to_arrow (feature = "arrow")
tests/data/                  # shared fixtures (used by CLI examples)
evals/                       # nonpublished evaluation workspace: validation harnesses, performance programs, allocation gates
fuzz/                        # libFuzzer targets (detached workspace; see fuzz/README.md)
```

## Things to know before editing

- **Workspace split.** Current 0.9 `powerio-matrix` depends on and re-exports
  `powerio`. The 1.0 restructure removes that edge: matrix depends on
  `powerio-tx`, `powerio-dist`, `powerio-prob`, and `powerio-core`; the top
  `powerio` facade re-exports matrix. Do not preserve
  current `crate::network` paths by introducing a facade cycle.
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
  and the writer returns it, so `parse → write → parse` keeps the exact bytes.
  In 1.0 the bytes belong to `PioModule.source`, not `BalancedNetwork`:
  every `mpc.*` field, in-matrix comments, and exact tokens like `7e-05`. Don't
  reformat through `f64` round-trips; don't drop fields the typed model ignores.
- **Two-tier fidelity rules.** Same format round trip is byte exact.
  Cross-format conversion keeps maximal fidelity and reports anything the target
  can't represent through `Diagnostic`; never drop it silently.
- **Adding a format.** A parser or writer produces the network, instance,
  solution, time series, scenario set, or other `PioModule` value declared by
  its supported profile. Use the
  opaque `Source`: `Source::open(path)` acquires a file or directory and
  `Source::from_bytes(name, bytes)` accepts memory input.
  Balanced formats
  meet at `BalancedNetwork`; conductor resolved formats meet at
  `MulticonductorNetwork`. Format enums are nonexhaustive and bindings use
  stable names rather than integer positions.
- **JSON transport.** `Network::to_json`/`from_json` (serde) is the structured
  transport; over the C ABI it is `pio_to_json`/`pio_from_json`. There is no
  format token for it: the `powerio-json` token was demoted in 0.7 and removed
  in 0.9, and the JSON classifier answers `model-json` for such a source. The
  current retained text is `#[serde(skip)]`, so JSON carries the tables, not the
  byte exact echo. The 1.0 parse API moves retained input to
  `PioModule.source`.
- **Distribution bindings stay lazy.** `pio_dist_parse_file` and
  `pio_dist_parse_str` return a live multiconductor handle. Julia display and
  scalar access use `pio_dist_summary_json`; `pio_dist_to_json` is the full
  element data and should only run when a caller asks for `net.data` or an
  element table.
- **`PioModule`.** `.pio.json` serializes one `PioModule<PioValue>`. The
  generic `PioModule<T>` has no marker trait bound and can hold application
  types outside the built in dynamic enum. It contains one typed `value` plus
  durable source map, diagnostic, and history records. Retained source is run
  time data and is not serialized. `TimeSeries<T>` and `ScenarioSet<T>` belong
  in the typed value rather than common module fields and compose as
  `ScenarioSet<TimeSeries<T>>`. The 0.9 `NetworkPackage` is gone: its
  decode survives crate private under `powerio::stored` for the one way
  upgrade. Typed entry selection must not serialize and clone the network.
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
- **Public DC matrices.** Match PowerModels exactly. `A` is branches by buses,
  `A[e, from] = +1`, and `A[e, to] = -1`. `SeriesSusceptance` uses
  `imag(inv(r + im*x))`, `TapAdjustedReactance` uses `-1/(x*tap)`, and
  `ReactanceOnly` uses `-1/x`. Build `B = A' * Diagonal(b) * A`,
  `Bf = Diagonal(b) * A`, and `p_shift = A' * (b .* shift)`. Then
  `p_bus = -B * va + p_shift` and `p_branch = -Bf * va + b .* shift`. `B` remains symmetric;
  phase shifts stay separate. Internal sparse factors can use positive graph
  weights, but their names must describe their solver role and sign conversion
  happens while filling the public output buffer.
- **DC OPF lives in `powerio-prob`.** `DcOpfInstance` keeps generator-space
  data (`generators: DcGeneratorData`); `nodal_generator_data()` scatters it to
  bus space through `C_g` for length-n `Q`, `c`, bounds, and `has_gen`. Cost map: MATPOWER `c2 p² + c1 p` → `q = 2c2`, `c = c1`, constant `c0` retained. Per-unit by default (`Units::PerUnit` scales `q` by `base²`, `c` by `base`).
- **A bus can host several generators.** `nodal_generator_data()` (DC and AC) sums the bounds, which is exact, and combines the cost curves by the parallel rule `q = 1/Σ(1/qᵢ)` in `powerio-prob/src/nodal.rs`, which is the curve of the least cost split and therefore an approximation. One generator at a bus passes through unchanged.
- **A leading cost coefficient can be a rounding artifact.** A model 2 row that states a linear curve often carries a quadratic term near `1e-17`. `GenCost::quadratic_with_constant_tol(tol)` takes the leading coefficients at or below `tol` off the row before it counts the row; `GenCost::LEADING_COEFF_TOL` is `1e-12`. `quadratic_with_constant` interprets the row as stored and the OPF builders call it.
- **`rate_a == 0` means unlimited.** `f_max`/`s_max` keep the zero. The opt-in `synthesize_unrated_limits` build option replaces it with `Branch::synthesize_rate_a`: the widest voltage phasor difference the terminal ceilings and the branch angle window allow, over `|Z|`, times the larger ceiling. The caller passes the window in radians and the method holds it at π; `angmin`/`angmax` are degrees in the neutral model and radians after `to_normalized`, so the builders convert them through `IndexedNetwork::angle_radians` and a raw case and its normalized form get the same bound.
- **`gen`/`gencost` are optional.** A power flow case with no `mpc.gen` parses with `gens` empty; the OPF builders return `Error::NoGenerators`.
- **Reference (slack) buses are a set, grounded one row/column each.** `IndexedNetwork::reference_bus_indices` returns every `BusType::Ref`; the matrix builders ground the whole set, so a network needs one reference *per connected component* (`IndexedNetwork::check_reference_coverage`). Several within one island is a distributed-slack solve. `reference_bus_index` is the exactly-one convenience query (errors otherwise) for the single-slack C/Python/gridfm paths. `DcOpfInstance`/`AcOpfInstance` carry the set as `ReferenceBuses`, which has no `first()`: a consumer picks `iter()` or the named `single()` (`Error::ReferenceBusCount`) rather than grounding one island of many by accident. Its serde form is the same array of dense indices.
- **PTDF/LODF need a solve.** They factor the reference grounded internal DC
  matrix, which is positive definite when every energized component has a
  reference, in `matrix::sensitivity`; no external solver dependency. Factor
  once and reuse scratch buffers. PTDF is dense `m×n`; sparse work computes
  selected columns or uses sparse factors rather than calling the complete PTDF
  sparse.
- **MTX output is lower triangle, 1 based, spec compliant.** `sprs::io::write_matrix_market_sym` writes the *upper* triangle, so `io::mtx::write_mtx` ships its own writer.
- **`CooBuilder`.** HashMap COO with O(nnz) inserts; replaces the old O(nnz²) Vec search.
- **TUI lives in the CLI crate.** `powerio-cli/src/tui/`, part of the `powerio` binary. Testable via `ratatui::backend::TestBackend`.
- **petgraph data.** `IndexedNetwork::to_petgraph()` currently returns
  `UnGraph<usize, usize>` where node weight = dense bus index and edge weight =
  branch index. Use it for connectivity and radial detection.
- **Format validation needs Julia.** `evals/validation/validate_powermodels.jl` and `validate_psse.jl` check the writers and parsers against PowerModels.jl; they don't run in plain `cargo test` (the all-pairs `powerio/tests/roundtrip_formats.rs` does).

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
