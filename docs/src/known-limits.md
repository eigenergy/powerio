# Known limits and versions

## Known limits

- Format profiles are bounded. PyPSA support covers the CSV electrical
  profile; multi carrier components, investment periods, and stochastic data
  are retained and reported rather than typed. Optional Python NetCDF/HDF5
  transports use this same profile; see [Sienna and pipeline exchange](sienna.md).
  Egret support is the scalar network profile with time series, with
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
  instance are not part of 0.12.
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
  Both are stable through 0.12.x.
- Python tables return dict rows; typed element views like Julia's are not
  offered yet.
- `serialize_diagnostics` returns the JSON text of a diagnostics array, where
  `serialize` returns an `EmitResult`: a diagnostics array is not a module
  document and has no artifacts or findings of its own.
- Public `powerio-matrix` signatures return `sprs::CsMat`, and some public
  coefficient types contain `num_complex::Complex64`. These dependencies are
  part of PowerIO's compatibility guarantees. Their pre-1.0 versions do not
  inherently prevent PowerIO 1.0, but changing their public types would break
  consumers. Before 1.0, choose either a documented commitment to those types
  throughout PowerIO 1.x or owned matrix/complex interfaces with explicit
  adapters. That decision remains open; updating a lockfile alone does not
  settle it.

## Path to 1.0

The 0.12 API separates source/provenance (`PioModule`) from electrical values,
keeps balanced and conductor-resolved networks distinct, and derives matrices
and problem instances explicitly. A new format maps to a shared model instead
of requiring pairwise converters. Owner-rooted C/Julia handles and lazy Python
access avoid using serialized IR as an internal binding protocol. These are
useful stable boundaries; a 1.0 promise still needs evidence for their semantics.

Before the next breaking bump, close the conventions and reference/slack
questions ([#700](https://github.com/eigenergy/powerio/issues/700),
[#713](https://github.com/eigenergy/powerio/issues/713)), decide the exposed
guarantees for numerical types above, and review the public 1.0 milestone's remaining
CGMES difference models, boundary assembly and DGS reader work
([#459](https://github.com/eigenergy/powerio/issues/459),
[#457](https://github.com/eigenergy/powerio/issues/457),
[#150](https://github.com/eigenergy/powerio/issues/150)). A feature can be explicitly
deferred by maintainers; it must not silently disappear from the milestone.
Require compatibility checks against the last published series, binding parity,
archived-schema reads, and independently validated conversion profiles.
Unsupported target features need truthful diagnostics, not an all-green matrix.

Keep one release series as the compatibility boundary. Do not introduce another
ABI counter or restart an independent IR counter: the C interface uses the
release series, and each published schema names that release, even when its payload layout is
unchanged. The release check requires the schema identifier to match the core
release version; prerelease suffixes remain in producer metadata. Published
snapshots remain immutable. This avoids a separately managed schema counter.
At 1.0 the stable series becomes 1.x rather than 0.12.x. This is a path to a
stable API, not a claim that today's outstanding decisions are already resolved.

## Versions

One PowerIO release version covers the Rust crates, the Python distribution,
and PowerIO.jl. The boundaries checked independently are:

| Boundary | Value in 0.12.x | Checked where | Moves when |
|---|---|---|---|
| PowerIO release | 0.12.z | the manifests, `powerio::VERSION`, `powerio.versions()`, `pio_version` | every release |
| C ABI | the release series, 0.12.x | `pio_version_numbers` against the header's `PIO_VERSION_*` at load | a release removes or changes a C symbol, which starts a new series |
| PowerIO IR | the release whose schema a document follows, `"0.12.0"` | the document header, `powerio::IR_VERSION` | every published release; supported types read within the same series, plus 0.11's `2` |
| Rust toolchain | 1.89 | `rust-version` in the workspace manifest, checked by CI | a dependency in the locked graph requires a newer compiler |
| Python | 3.11 or later | `pyproject.toml` | a dependency drops a version |

Within the 0.12.x series, releases are compatible: the public Rust, Python,
and Julia APIs only gain items, the C ABI only gains symbols, and a release
reads older documents in the series. An older reader can read newer documents
of types it supports and explicitly refuses unknown structural types. A break
to an existing type waits for the next series. Before 1.0 a series is one minor release, as Cargo defines it.
CI checks the publishable crates against the latest release tag with
cargo-semver-checks (`scripts/check-semver.sh`), and the C header against the
latest release header (`scripts/check-capi-header.py`); a pull request that
carries a break is labeled `semver-major`. See [PowerIO IR](pio-json-schema.md)
for the document rule and [From 0.11 to 0.12](migration-0.12.md) for what
changed.
