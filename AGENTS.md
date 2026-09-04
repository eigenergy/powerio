# AGENTS.md

Guidance for agents working in this repo.

## Purpose

A Cargo workspace of Rust crates plus a Python package. PowerIO parses power
system data into typed values, converts between formats, and emits sparse
matrices and graph data for downstream solvers. It feeds the GridFM ML
pipeline. The files under `arch-v1/` and `docs/design/` record the design
work that led to the 0.10 beta and the 0.11 API. They are dated evidence, not
current API authority: read them for the reasoning behind a public name, then
check the current source, tests, and release notes.

- **`powerio-core`**: the shared foundation: `Source`, `FormatId`,
  `Diagnostic`, `Error`, `PioModule<T>`, common module records,
  `TimeSeries<T>`, `ScenarioSet<T>`, and output destination types. It has no
  electrical network, matrix, or solver dependencies.
- **`powerio-tx`**: the format neutral `BalancedNetwork`, the balanced format
  readers and writers, normalization, and derived indexed views.
- **`powerio-dist`**: the multiconductor distribution model
  (`MulticonductorNetwork`) with the OpenDSS `.dss`, PMD JSON, and BMOPF JSON
  converters. It does **not** depend on the transmission crate; both share
  `powerio-core`. BMOPF network decoding lives here; construction of the
  resulting `McAcOpfInstance` lives in `powerio-prob`.
- **`powerio-prob`**: operating points, updates, calculation instances, and
  solutions over both network families. It stays matrix free; sparse
  operators and the DC OPF bundle writer live in `powerio-matrix`.
- **`powerio-matrix`**: sparse matrices and graph data built on the component
  crates. It must not depend on or re-export the `powerio` facade, which
  would create a dependency cycle.
- **`powerio`**: the facade. It owns `PioValue`, `parse`, `emit`,
  `serialize`, `deserialize`, format dispatch, the PowerIO IR document, and
  the public re-exports. The retired `powerio-pkg` and `powerio-diag`
  boundaries must not be recreated.
- **`powerio-cli`**: the `powerio` binary: the clap CLI and the ratatui TUI
  over the facade and its component crates.
- **`powerio-py`**: the PyO3 extension behind the `powerio` Python package
  (`python/powerio/`); it hands back COO triplets that SciPy assembles.
- **`powerio-capi`**: C ABI 7 over `powerio` (`pio_*`, header `powerio.h`)
  for C, C++, Julia, and other FFI users. `pio_parse` returns a module
  handle, `pio_module_value` an owner rooted value handle, and the typed
  accessors return borrowed views. ABI 7 exports one fixed symbol set; only
  the `gridfm` cargo feature changes the build, and the other feature names
  the release build passes gate nothing.

`BalancedNetwork` and `MulticonductorNetwork` are the two electrical network
types. The normalized solver tables and dense row arrays are internal
compiler data. `IndexedNetwork` stays a public derived index view in 0.11
because the matrix builders and downstream consumers take it directly.

Formats. MATPOWER `.m`, PSS/E `.raw` (revisions 32 to 35 in, 33 to 35 out),
PSS/E RAWX 35, PowSybl XIIDM and JIIDM (1.0 to 1.17 in, 1.17 out), CIM CGMES
(2.4.15 and 3.0 in, 3.0 out), ENTSO-E UCTE-DEF, PowerWorld `.aux`, PSLF
`.epc`, PowerModels JSON, Egret JSON, pandapower JSON, PyPSA CSV directories,
and Surge JSON parse and write. The IEEE Common Data Format, DOE GO Challenge
3 problem data, DeepMind OPFData JSON, and PowerWorld `.pwb` are read only;
a complete `AcScucSolution` writes the official GO Challenge 3 solution
file. A PowerWorld `.pwd` display parses to `PioValue::GeoLayer`. GridFM
Parquet directories parse and write behind the `gridfm` feature. OpenDSS
`.dss`, PMD engineering JSON, and BMOPF JSON meet at `MulticonductorNetwork`.
PowerIO IR moves through `serialize` and `deserialize` and is not a grid
exchange format; `parse` refuses it. Every balanced format maps to
`BalancedNetwork`, so a new format needs one reader and one writer rather
than pairwise converters. PyPSA support is the documented CSV electrical
profile: a directory parses to `BalancedNetwork`,
`TimeSeries<BalancedNetwork>`, or, when only a complete electrical state
varies, `TimeSeries<OperatingPoint<BalancedNetwork>>`; components,
intertemporal data, and stochastic data outside that profile are retained
for same format writing and reported before cross format projection.

Matrix outputs (`powerio-matrix`):
- MATPOWER FDPF `Bp` (`bprime`): `-Im(Y_bus)` after clearing bus shunts and
  line charging and setting tap magnitudes to one. XB clears resistance.
  Phase shifts remain, matching MATPOWER `makeB`.
- MATPOWER FDPF `Bpp` (`bdoubleprime`): `-Im(Y_bus)` after clearing phase
  shifts. BX clears resistance. Line charging, bus shunts, and tap magnitudes
  remain.
- `Re(Y_bus)`, `-Im(Y_bus)` (full); the AC power flow Jacobian.
- LACPF block `[[G, -B], [-B, -G]]` (linear AC power flow, flat start,
  2n×2n, indefinite).
- Adjacency (`MatrixKind::Adjacency`); PTDF and LODF (`sensitivities`).
- DC matrix data follows PowerModels: incidence `A` is branches by buses with
  `+1` at the from bus and `-1` at the to bus, `B = A' * Diagonal(b) * A`,
  and `Bf = Diagonal(b) * A`. Phase shift injection stays separate.
- petgraph `UnGraph<bus_idx, branch_idx>` data plus connectivity and radial
  diagnostics.
- GridFM Parquet directory writing through `gridfm-datakit` compatible
  tables; GridFM parsing reuses one balanced network identity set across
  scenarios rather than cloning one network per scenario.

## Commands

```
cargo build --release        # default workspace members except powerio-py and powerio-capi
cargo test                   # default workspace members
cargo test -p powerio-capi   # the C ABI tests (not in default-members)
bash scripts/ci-clippy.sh    # full CI clippy matrix; run before pushing Rust/C ABI changes
cargo fmt --all --check      # rustfmt is enforced (edition 2024)

# CLI (the binary is `powerio`):
powerio                                                   # TUI
powerio convert tests/data/case14.m --to psse -o case14.raw
powerio summary tests/data/case14.m
powerio serialize tests/data/case14.m -o case14.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio batch -i tests/data -o out --matrices bprime,bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio gridfm tests/data/case14.m -o out      # GridFM Parquet directory
powerio gen --topology lattice --n 1024 -o out
powerio geo extract case.aux -o layer.geo.json

# C ABI (cdylib + staticlib; header powerio-capi/include/powerio.h):
cargo build -p powerio-capi
cargo build -p powerio-capi --features gridfm  # the one build option

# Python (the PyO3 crate needs libpython, so it is NOT in default-members):
cargo build -p powerio-py                      # plain cargo build of the extension
maturin build --release -o dist                # a wheel, from the repository root
pip install dist/*.whl && pytest python/tests  # an editable install is shadowed by powerio/
```

## Release flow

PowerIO releases are tag driven.

1. Wait for `main` CI to pass on the merge commit that should become the
   release, and make sure `CHANGELOG.md` has a section headed exactly
   `## X.Y.Z`; the tag workflow copies it into the draft release body and
   fails without it.
2. Obtain maintainer approval of the final cross repository diff and test
   packet. Do not mark the PowerIO.jl release intent ready or create a tag
   before this approval.
3. Merge the reviewed PowerIO.jl release intent for the same version before
   tagging PowerIO. PowerIO.jl's `Project.toml`, top `CHANGELOG.md` section,
   and `.github/powerio-release.toml` must agree on the Julia version and
   `vX.Y.Z` PowerIO tag. The intent is marked ready only after its canonical
   source digest matches the reviewed PowerIO.jl tree.
4. Check that the tag does not already exist, then create an annotated tag on
   `origin/main` and push it:

   ```
   git fetch origin main --tags
   git ls-remote --tags origin refs/tags/vX.Y.Z
   git tag -a vX.Y.Z origin/main -m vX.Y.Z
   git push origin vX.Y.Z
   ```

5. `.github/workflows/release-binaries.yml` runs on tag pushes. Its binding
   gate tests PowerIO.jl `main` against the tagged library before it builds
   the `powerio-capi` release tarballs for `aarch64-apple-darwin`,
   `aarch64-linux-gnu`, `x86_64-apple-darwin`, `x86_64-linux-gnu`, and
   `x86_64-w64-mingw32`, with the release features
   `arrow,matrix,gridfm,dist,prob`.
6. That workflow creates or updates a **draft** GitHub release and attaches
   the five binary assets. Do not expect a draft release to exist before the
   tag workflow runs.
7. A human inspects and publishes the draft release. Publishing also starts
   the crates.io workflow, which verifies the workspace package set and
   publishes `powerio-core`, `powerio-tx`, `powerio-dist`, `powerio-prob`,
   `powerio-matrix`, `powerio`, and `powerio-cli` in dependency order. A
   rerun skips versions already present on crates.io.
8. Publishing the release triggers `.github/workflows/notify-powerio-jl.yml`.
   If `POWERIO_JL_DISPATCH_TOKEN` is configured, it sends a
   `powerio-release` repository dispatch to `eigenergy/PowerIO.jl`. If the
   token is absent, the PowerIO.jl daily schedule or manual dispatch is the
   fallback.
9. PowerIO.jl's `.github/workflows/update-artifacts.yml` accepts only the tag
   named by the ready release intent. It verifies the published release and
   exact five assets, registry order, absence of an open `artifacts/*` PR,
   the ABI handshake, schema report, and full Julia tests. It may change only
   `Artifacts.toml`. After confirming that PowerIO.jl `main` is still the
   reviewed base SHA, it commits that one file to `main` and dispatches
   registration for the exact resulting SHA. A schedule is the backstop and a
   manual dispatch retries the same intent; neither can override its version,
   tag, changelog, or source digest.

## Layout

```
powerio-core/src              module.rs (PioModule<T>), source.rs, output.rs,
                              diagnostic.rs, codes.rs, error.rs, records.rs,
                              time_series.rs, scenario.rs, component_id.rs,
                              validation.rs (limits), bounded.rs, nonfinite.rs
powerio-tx/src                network.rs (BalancedNetwork and its tables),
                              indexed.rs, normalize.rs, operations.rs, dc.rs,
                              gen_cost.rs, geo/ (layer.rs, pwd.rs), collect.rs,
                              diagnostics.rs, version.rs
powerio-tx/src/format         mod.rs (parse, emit, TargetFormat, EmitOptions),
                              routing.rs (tokens, JSON classification), xml.rs,
                              matpower/, psse.rs, rawx.rs, xiidm.rs, cgmes/,
                              ucte/, ieee_cdf.rs, powerworld/ (aux, pwb, pwd),
                              pslf.rs, powermodels.rs, egret.rs, pandapower.rs,
                              pypsa.rs, surge.rs, goc3.rs, opfdata.rs
powerio-dist/src              model.rs, convert.rs, dss/, pmd/, bmopf/, graph.rs,
                              geo.rs, diagnostics.rs, error.rs
powerio-prob/src              operating/, instance/, solution/, update.rs,
                              reference.rs, goc3/, formats/ (goc3, opfdata, pypsa)
powerio-matrix/src            matrix/ (bprime, bdoubleprime, ybus, lacpf,
                              incidence, adjacency, laplacian, sensitivity,
                              multiconductor, triplet), dc_operators.rs,
                              ac_jacobian.rs, dcopf/ (prep, nodal, limits, bundle),
                              acopf.rs, opf.rs, io/ (mtx, meta, sensitivity, gridfm),
                              pipeline.rs, synth/
powerio/src                   lib.rs (parse, emit), value.rs (PioValue), ir.rs,
                              stored/ (dto.rs, convert.rs), formats.rs, transform.rs,
                              write.rs, gridfm.rs, dist_geo.rs, codes.rs
powerio-cli/src               main.rs (clap), cases.rs, module_io.rs, codes.rs,
                              invariants.rs, corpus/, tui/
powerio-py/src/lib.rs         the PyO3 extension, module `powerio._powerio`
python/powerio                the package (__init__.py, dist.py, mcp/, stubs)
powerio-capi/src              lib.rs (pio_*), diagnostics.rs; include/powerio.h
tests/data                    shared fixtures
evals/                        validation, PowSybl, performance, and allocation harnesses
fuzz/                         libFuzzer targets (detached workspace)
docs/                         the mdBook guide, the schema archive, release notes
```

## Things to know before editing

- **Workspace split.** `powerio-matrix` depends on `powerio-core`,
  `powerio-tx`, `powerio-dist`, and `powerio-prob`; the `powerio` facade
  re-exports matrix types behind the `matrix` feature. Do not introduce a
  facade cycle.
- **Clippy must match CI.** Root `cargo clippy --all-targets` skips feature
  combinations and the PyO3 extension. Run `bash scripts/ci-clippy.sh` before
  pushing Rust, C ABI, matrix, feature, or Python extension changes.
- **One Python wheel (maturin mixed layout).** `powerio-py/` compiles to the
  native module `powerio._powerio`; `python/powerio/` is the pure Python
  wrapper that turns COO triplets into `scipy.sparse` and NetworkX. The
  triplets cross as plain Python lists, so `import powerio` pulls in nothing
  but the interpreter; SciPy, NumPy, NetworkX, and Polars are extras.
- **Lossless same format emission.** A parse retains the source bytes on the
  module and emission returns them, so `parse` then `emit` keeps the exact
  bytes. The retained bytes do not belong to the network value. Do not
  reformat through `f64` round trips or drop fields the typed model ignores.
- **Two tier fidelity.** Same format round trip is byte exact. Cross format
  emission keeps maximal fidelity and reports anything the target cannot
  represent through a `Diagnostic`; never drop it silently. The conversion
  matrix test in `powerio-cli/tests/conversion_matrix_report.rs` pins the
  expected warning counts; an intentional change edits the baseline in the
  same PR.
- **Adding a format.** A reader produces the value its supported profile
  declares. Balanced formats meet at `BalancedNetwork`; conductor resolved
  formats meet at `MulticonductorNetwork`. Register the token in
  `powerio-tx/src/format/routing.rs`, the diagnostics family in
  `powerio-tx/src/diagnostics.rs`, and the facade metadata in
  `powerio/src/formats.rs`. Format enums are nonexhaustive and bindings use
  stable names rather than integer positions.
- **PowerIO IR.** `serialize` and `deserialize` move `PioModule<PioValue>`.
  Retained source bytes are not serialized. The document identity is
  `pio-ir` with integer generation `2`; `IR_VERSION` and `IR_MIN_VERSION` in
  `powerio/src/lib.rs` state the window, and every 0.11.x release reads
  every generation the line wrote. The DTOs in `powerio/src/stored/dto.rs`
  are the document layout; runtime types never derive it. Regenerate
  `docs/schema/pio-ir/2/schema.json` when the DTOs change.
- **Bindings stay typed and lazy.** C calls `pio_module_value`, checks the
  structural type, and requests an owner rooted typed handle. Python reads
  `module.value`; Julia dispatches on `PioModule{T}`. Typed access does not
  serialize or clone a module value.
- **Bus IDs.** Source bus ids are the source's own; `IndexedNetwork::bus_index(id)`
  is the only mapping into dense `[0, n)`. Do not clamp out of range; return
  `Error::UnknownBus`.
- **`BR_B` is already per unit.** Never divide by `base_mva` again.
- **`tap == 0` means `tap = 1`.** Use `Branch::calc_effective_tap()`.
- **MATPOWER FDPF matrices.** `calc_bprime_matrix` follows MATPOWER `makeB`
  `Bp` and `calc_bdoubleprime_matrix` follows `Bpp`, as listed under matrix
  outputs above. `Y_bus` keeps taps and shifts.
- **Public DC matrices.** Match PowerModels exactly. `A` is branches by
  buses, `A[e, from] = +1`, `A[e, to] = -1`. `SeriesSusceptance` uses
  `imag(inv(r + im*x))`, `TapAdjustedReactance` uses `-1/(x*tap)`, and
  `ReactanceOnly` uses `-1/x`. `B = A' * Diagonal(b) * A`,
  `Bf = Diagonal(b) * A`, `p_shift = A' * (b .* shift)`,
  `p_bus = -B * va + p_shift`, `p_branch = -Bf * va + b .* shift`. The DC
  OPF bundle files use positive susceptance magnitudes and a bus by branch
  incidence; the sign conversion happens while filling the output.
- **DC OPF preparation lives in `powerio-matrix/src/dcopf/`.** `prep.rs`
  keeps generator space parameters, and `calc_nodal_generator_data()`
  scatters them to bus space through `C_g`. Cost map: MATPOWER
  `c2 p² + c1 p` becomes `q = 2c2`, `c = c1`, with `c0` retained. Per unit by
  default: `Units::PerUnit` scales `q` by `base²` and `c` by `base`.
- **A bus can host several generators.** `nodal.rs` sums the bounds, which
  is exact, and combines the cost curves by the parallel rule
  `q = 1/Σ(1/qᵢ)`, the curve of the least cost split, which is an
  approximation. One generator at a bus passes through unchanged.
- **A leading cost coefficient can be a rounding artifact.** A model 2 row
  that states a linear curve often carries a quadratic term near `1e-17`.
  `GenCost::LEADING_COEFF_TOL` (`1e-12`) is the tolerance the quadratic
  projections apply before counting the row's degree.
- **`rate_a == 0` means unlimited.** The opt in `synthesize_unrated_limits`
  build option replaces it with `Branch::synthesize_rate_a`. `angmin` and
  `angmax` are degrees in the neutral model and radians after
  `to_normalized`; the builders convert so a raw case and its normalized form
  get the same bound.
- **`gen` and `gencost` are optional.** A power flow case with no `mpc.gen`
  parses with an empty generator table; the OPF builders return
  `Error::NoGenerators`.
- **Reference buses are a set, grounded one row and column each.**
  `IndexedNetwork::reference_bus_indices` returns every `BusType::Ref`; the
  builders ground the whole set, so a network needs one reference per
  connected component (`IndexedNetwork::check_reference_coverage`).
  `reference_bus_index` is the exactly one convenience query. Instances
  carry the set as `ReferenceBuses`, which has no `first()`: a consumer
  picks `iter()` or `single()` (`Error::ReferenceBusCount`).
- **PTDF and LODF need a solve.** `matrix::sensitivity` factors the
  reference grounded DC matrix; `SensitivitySolver` is `Auto`, `Dense`, or
  `Sparse`. Factor once and reuse scratch buffers.
- **MTX output is lower triangle, 1 based.** `sprs` writes the upper
  triangle, so `io::mtx::emit_mtx` ships its own emitter.
- **`CooBuilder`** in `matrix/triplet.rs` is HashMap COO with O(nnz)
  inserts.
- **The TUI lives in the CLI crate** under `powerio-cli/src/tui/` and is
  testable through `ratatui::backend::TestBackend`.
- **petgraph data.** `IndexedNetwork::to_petgraph()` returns
  `UnGraph<usize, usize>` with the dense bus index as node weight and the
  branch index as edge weight.
- **Format validation needs Julia and Python oracles.** The harnesses under
  `evals/validation/` and `evals/powsybl/` check the readers and writers
  against PowerModels.jl, PowSybl, and the Python tools; they do not run in
  plain `cargo test`. The all pairs `powerio-tx/tests/roundtrip_formats.rs`
  does.

## Test fixtures

`tests/data/case{9,14,30,57,118}.m` and `case2869pegase.m` are vendored
verbatim from `https://github.com/MATPOWER/matpower/tree/master/data`
(BSD-3). Also `t_case9_dcline.m`, `pglib/` (PGLib OPF), `psse/*.raw`, the
PowSybl XIIDM and CGMES fixtures under `tests/data/xiidm` and
`tests/data/cgmes` (MPL-2.0), and the distribution fixtures under
`tests/data/dist`. `tests/data/pandapower/example.json` was written by
pandapower 3.2.2 and `tests/data/pypsa/example/` by PyPSA 1.2.2; both are
committed byte exact as the tool wrote them.

Use the smallest fixture that exercises the behavior under test. Never add a
fixture larger than 100 KiB unless the user explicitly approves that exact
file after being told its byte count, line count, source, license, and
effect on the pull request. Approval of a broader implementation plan does
not authorize vendoring its test data. Do not commit a fixture without a
license that permits redistribution. Prefer synthetic fixtures unless byte
exact source fidelity is the behavior under test. Run `wc -lc` and inspect
`git diff --stat` before committing any new fixture.

## Relationship to GridFM

PowerIO is the Rust data layer beneath `gridfm-datakit` (Python, scenario
generation) and `gridfm-graphkit` (PyTorch Geometric, GNN training). The
`gridfm` subcommand (`io::gridfm`, `--features gridfm`) emits the
`bus_data`, `gen_data`, `branch_data`, and `y_bus_data` Parquet tables that
match gridfm-datakit's column schema, under `<out>/<case>/raw/`, so
gridfm-graphkit's `HeteroGridDatasetDisk` loads PowerIO output directly.
PowerIO has no solver, so a case is one snapshot: voltages and dispatch are
the case's stored values and branch flows are computed from them. A scenario
batch (`emit_gridfm_batch` and `GridfmSnapshot`, or several `gridfm` CLI
inputs) row stacks snapshots that share one base element set, keyed by the
`scenario` column.
