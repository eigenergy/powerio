# Known limits and versions

## Known limits

- Format profiles are bounded. PyPSA support covers the CSV electrical
  profile; multi carrier components, investment periods, and stochastic data
  are retained and reported rather than typed, and PyPSA NetCDF does not
  parse. Egret support is the scalar network profile with time series, with
  unit commitment fields outside it. OpenDSS support is the static circuit;
  load shapes and solve instructions are retained and reported. Distribution
  CIM support is the CIM100 node breaker profile with per phase equipment and
  the CIM16 LV feeder profile; controls, distributed energy resources, wire
  geometry, and limits are reported rather than mapped. A ZIP archive or a
  folder of related files is read through an [input collection](collections.md).
- DOE GO Challenge 3 problem data, DeepMind OPFData, PowerWorld PWB, and the
  IEEE Common Data Format are parse only, though a complete `AcScucSolution`
  does emit the official GO Challenge 3 solution file.
- PowerIO does not solve anything; instances feed external solvers.
- Transient-stability `.dyr` files parse into a loss-preserving `DynamicCase`
  and pair with a steady-state `BalancedNetwork` through `DynamicStudy`. The
  current link check validates bus references only; model-specific device
  schemas, typed machine/control views, cross-format dynamics conversion, and
  PowerWorld AUX dynamics views remain future work. See [Dynamics and DYR](dynamics.md).
- Balanced to multiconductor construction, load linearized multiconductor
  admittance from an operating point, and a general multi period planning
  instance are not part of 1.0.
- The multiconductor to balanced transformation is Rust and Python only.
  Multiconductor admittance assembly,
  `powerio_matrix::calc_multiconductor_admittance_matrix`, is Rust only.
- Only the MATPOWER, PSS/E RAW, UCTE-DEF, and IEEE CDF readers attach source
  spans, marking the record each finding is about. Findings from the other readers, from
  transformations, and from writers have no byte range, and the text rendering
  of a diagnostic is still `CODE: message`.
- There is no one call `convert` in the library; you compose `parse` with
  `emit`. The command line does have `powerio convert`.
- The MATPOWER, PSS/E, and PowerWorld AUX readers tokenize without owned
  strings. PyPSA CSV, PSLF, and OpenDSS tokenize through owned strings, and
  the Egret, GO Challenge 3, OPFData, and pandapower readers decode through a
  generic JSON value tree first.
- Two sets of field names coexist, one per kind of binding. Rust records,
  Python dict rows, and PowerIO IR use the data model's names, derived from
  MATPOWER (`vm`, `pg`, `rate_a`, `tap`); the C ABI and Julia spell out the
  quantity and unit (`vm_pu`, `active_power_mw`, `rate_a_mva`, `tap_ratio`).
  Both are stable through 1.x.
- Python tables return dict rows; typed element views like Julia's are not
  offered yet.
- `serialize_diagnostics` returns the JSON text of a diagnostics array, where
  `serialize` returns an `EmitResult`: a diagnostics array is not a module
  document and has no artifacts or findings of its own.
- Public `powerio-matrix` signatures return `sprs::CsMat`, and
  `IndexedNetwork::to_petgraph` returns a `petgraph::graph::UnGraph`. Both
  crates are before their own 1.0, so moving either to an incompatible
  release is a breaking PowerIO change, made only in a new major release.

## Versions

One PowerIO release version covers the Rust crates, the Python distribution,
and PowerIO.jl. The boundaries checked independently are:

| Boundary | Value in 1.x | Checked where | Moves when |
|---|---|---|---|
| PowerIO release | 1.y.z | the manifests, `powerio::VERSION`, `powerio.versions()`, `pio_version` | every release |
| C ABI | the release series, 1.x | `pio_version_numbers` against the header's `PIO_VERSION_*` at load | a release removes or changes a C symbol, which starts a new series |
| PowerIO IR | the release whose schema a document follows, `"1.0.0"` | the document header, `powerio::IR_VERSION` | every release that changes the schema; a reader accepts its own series and 0.11's `2` |
| Rust toolchain | 1.89 | `rust-version` in the workspace manifest, checked by CI | a dependency in the locked graph requires a newer compiler |
| Python | 3.11 or later | `pyproject.toml` | a dependency drops a version |

Within the 1.x series, releases are compatible: the public Rust, Python,
and Julia APIs only gain items, the C ABI only gains symbols, and a release
reads every PowerIO IR document the series wrote. A break waits for 2.0.
CI checks the publishable crates against the latest release tag with
cargo-semver-checks (`scripts/check-semver.sh`), and the C header against the
latest release header (`scripts/check-capi-header.py`); a pull request that
carries a break is labeled `semver-major`. See [PowerIO IR](pio-json-schema.md)
for the document rule and [From 0.11 to 1.0](migration-1.0.md) for what
changed.
