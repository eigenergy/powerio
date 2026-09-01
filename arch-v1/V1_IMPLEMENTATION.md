# PowerIO 1.0 implementation

Status: historical implementation plan from 2026-08-25. It records the
technical order and validation gates that led to the PowerIO 0.10 beta and
informed the final 1.0 corrections. It is not current API authority or an
execution guide.

`PioModule<T>` is the accepted top level compiler type. Every successful parse
returns one. `.pio.json` is its versioned dynamic serialization, not a case
format or a direct dump of Rust memory.

## Release outcome

The plan defined completion as:

- the public networks, problem instances, solutions, modules, diagnostics,
  sources, and matrix data match the
  architecture documents;
- every format states its supported profile and types every field inside it;
- the PyPSA CSV electrical profile maps a static snapshot to
  `BalancedNetwork`, supported input series to
  `TimeSeries<BalancedNetwork>`, and complete state-only series to
  `TimeSeries<OperatingPoint<BalancedNetwork>>`; it diagnoses data outside the
  profile and retains that data for exact same format writing;
- OpenDSS distinguishes circuit data, schedules and calculation instructions, and
  solved QSTS state sequences;
- Egret preserves `system.time_keys` and time series values for supported
  network fields;
- GridFM preserves its scenarios without allocating an independent network
  for every scenario;
- operations exposed on more than one of Rust, C, Python, Julia, CLI, and MCP
  use the same public meanings;
- Tellegen and the independent Julia tools consume PowerIO instances without a
  second public network model;
- every issue in the 1.0 milestone is closed by verified code or an explanatory
  scope correction;
- correctness, allocation, memory safety, fuzz, and release gates pass.

## Preimplementation evidence

The source profile review, adversarial API audit, and `arch-v1/prototype/`
closed the product decisions. The one crate and multi-crate prototypes compile
an unconstrained `PioModule<T>`, flat `PioValue`, recoverable facade
free-function narrowing, immutable cheap to clone network handles, private shared temporal data,
`TimeSeries<OperatingPoint<MulticonductorNetwork>>`, one stored document
version, and owned path and named memory destinations. Public traits for memory representation,
`BalancedNetworkData`, per value schema versions, `powerio-pkg`, and a separate
`powerio-diag` leaf were rejected in that design.

The GitHub trackers listed in `V1_ISSUE_AUDIT.md` were created on 2026-08-25.
They preserve the issue scope used by this plan.

## Historical implementation sequence

### Wave 0: baseline and issue coverage

1. Inspect status in PowerIO, PowerIO.jl, Tellegen, Egret, PyPSA, PowerModels,
   PowerModelsDistribution, BMOPFTools, ExaModelsPower, and bmopf-report. Fetch
   current remote refs without changing checked out files. Run tests in clean
   worktrees or at recorded revisions.
2. Record exact revisions used by evaluation adapters.
3. Run the current Rust, C, Python, Julia, documentation, and conversion suites.
4. Record parser allocation counts, allocated bytes, peak resident memory, and
   wall time on the small and large evaluation cases.
5. Record current sparse matrix allocations for DC incidence, PTDF, LODF, and
   multiconductor admittance construction.
6. Build an issue coverage table covering every 1.0 issue and every missing
   tracker.
7. Confirm each post 1.0 issue still belongs outside the release.
8. Apply `V1_TERMINOLOGY.md` to every public Rust item, C header, Python and
   Julia binding, CLI command, README, generated API page, changelog, test, and
   open issue. One concept must have one name in every language.
9. Verify that experimental and planned parsers can acquire their named input
   buffers through the opaque `Source` and produce an existing network,
   instance, solution, time series, or scenario set through the settled top
   level parse shape. Formats that use several files must not require new
   public source variants.

No performance claim is accepted without before and after measurements on the
same input and build profile.

### Wave 1: foundation

#### Modules, diagnostics, and sources

- Apply the accepted crate layout first: create one `powerio-core` foundation
  for `Source`, diagnostics, `Error`, `PioModule<T>`, common records, repeated
  value containers, and output types; rename the balanced crate to
  `powerio-tx`; retire `powerio-diag` and `powerio-pkg`; and make the short name
  `powerio` the entry facade that owns `PioValue`, `PioValueKind`, universal
  dispatch, and `.pio.json`. `powerio-tx` and `powerio-dist` both depend on
  `powerio-core` but never on each other.
- Replace the public coded finding name with `Diagnostic`.
- Record numerical repairs as structured descriptive history rather than a
  parallel common record family. Replayable revisions need a typed value.
- Keep the mutable diagnostic collector private.
- Rename `DiagnosticStage::Lower` and the public `LOWER` code namespace to
  `DiagnosticStage::Transform` and `TRANSFORM`.
- Replace the `UnknownFormat` error category with `Request`; keep the five
  stable categories `Io`, `Request`, `Parse`, `Data`, and `Output`.
- Implement unconstrained `PioModule<T>` with borrowed `value()` and consuming
  `into_value()` accessors. Do not add a public marker trait or sealing module.
  Runtime module structs do not derive the `.pio.json` wire layout.
- Implement one facade `parse(Source) -> PioModule<PioValue>` operation.
- Make `Source` an opaque owner or provider of named immutable buffers.
  Implement `Source::open(path)` for filesystem acquisition and
  `Source::from_bytes(name, bytes)` for memory input. Permit memory mapping and
  lazy directory acquisition without exposing public file or directory
  variants.
- Implement `Source::with_format` as an explicit parser selection. Without it,
  `parse` detects the format. The selected parser still validates the input.
  Store an open validated `FormatId` so `powerio-core` does not depend on
  either network crate's format enum.
- Add record preserving `PioModule::map_value` in `powerio-core`. In the
  facade, implement sealed behavioral `FromPioValue` once per registered
  `PioValue` variant and expose the advanced owned conversion
  `module.into_typed::<T>()`. Reverse conversion uses
  `module.map_value(PioValue::from)`. A successful conversion moves the value,
  source owner, and common records with no allocation. `ValueKindMismatch`
  reports the expected and actual `PioValueKind`, owns the original
  `PioModule<PioValue>`, and exposes `into_module()` so a failed assertion never
  destroys a parsed value. It projects to the `Request` category in common
  error reporting and bindings.
- Store successful parse diagnostics on the module and return failed operations
  in an opaque common `Error` type that
  implements `std::error::Error` and retains an underlying cause. Do not add a
  public `ParseError`; callers branch on the diagnostic code or `Io`, `Request`,
  `Parse`, `Data`, and `Output` category.
- Use `Error`, `Warning`, `Remark`, and `Note` as public diagnostic severities.
  A remark is a standalone message about a successful operation; a note adds
  context to another diagnostic. A module can contain an error diagnostic when
  it has a representable value that fails validation. Require a returned
  `Error` to contain at least one error severity diagnostic.
- Move retained source ownership out of both network types.
- Convert `BalancedNetwork` and `MulticonductorNetwork` into immutable cheap
  to clone owning handles over private shared tables. Cloning a handle
  must not clone table allocations. Instances store the network handle by
  value rather than wrapping it in a second public `Arc`; solutions do the same
  with immutable instance handles. Keep the choice of whole value or table
  granular internal sharing private and settle it with allocation benchmarks.
- Store retained source optionally on the module; parsed modules have it and
  modules constructed in memory do not.
- Share immutable source bytes with one owner and no repeated byte buffers.
- Store diagnostic locations as source buffer identifiers and byte spans. A
  successful module or failed `Error` retains the shared source owner; do not
  build self-referential structs.
- Preserve exact same format writing through an unchanged parsed module.
- Verify every successfully parsed or transformed module without repairing it.
- Fix version diagnosis and diagnostic aggregation behavior.

Issue coverage: #375 and #377, plus the new public parse and diagnostic tracker.

#### Repeated state data

- Port the audited prototype. Implement generic `TimePoint`, `TimeSeries<T>`,
  and `ScenarioSet<T>` in `powerio-core`. Implement `OperatingPoint<N>` and type
  specific operating point series constructors in `powerio-prob`, which can
  depend on both network crates. A time series is ordered; a scenario set contains named
  alternatives with no implied order; alternatives over time use
  `ScenarioSet<TimeSeries<T>>`.
- Keep generic container fields private and make `get` return `&T`. Do not
  expose `SeriesElement`, `ScenarioElement`, associated types describing memory
  representation, or an auxiliary public data type. A concrete
  `OperatingPoint<N>` can be a small owning
  handle into shared contiguous columns, sparse changes, and one immutable
  network handle. Retaining that point after dropping the collection must not
  materialize or copy a network.
- Give `ScenarioSet<T>` stable `ScenarioId` lookup. Preserve source order for
  deterministic writing without making position semantic identity. Require
  probabilities for every scenario or none and validate their range and sum.
  IDs are case sensitive and are never normalized. Build an internal ID index;
  lookup must not scan linearly.
- `OperatingPointData` is a provisional private name in the prototype, not a
  1.0 API name. Prefer separate private balanced and multiconductor types if
  implementation does not need one umbrella enum. If generic code still needs
  the enum, keep the private name `OperatingPointData`. Revisit it without a
  public API change once the complete operating point fields are implemented.
- Make matrix and instance consumers borrow concrete public values or narrow
  public read capabilities that express electrical semantics, not memory layout.
- Do not wrap a deterministic series in `ScenarioSet<T>` or represent
  security constrained contingencies as scenarios.
- Resolve stable element identities once and reuse the base network. Invalidate
  cached structure only when topology or parameters require it.
- Do not label time varying bounds, availability, commitment, reserves, and
  costs as operating points. Do not add a public `InvestmentPeriod` type or
  common module field; preserve source specific planning data for exact writing
  until a calculation type gives it meaning.

Issue coverage: #196 and the new time and scenario representation tracker.

#### Instances and solutions

- Implement `DcPfInstance`, `AcPfInstance`, `DcOpfInstance`, `AcOpfInstance`,
  `McAcPfInstance`, `McAcOpfInstance`, and `AcScucInstance`.
- Keep instance fields private and expose borrowed `network()` accessors.
- Share immutable networks rather than copying them into each instance.
- Keep generator cost curves on `BalancedNetwork`; store the selected objective
  on the OPF instance.
- Keep physical limit values on the network. Store active constraint selections
  by stable element identity and calculation data not reusable across problems
  on the OPF instance.
- Implement typed objective terms and a consuming builder style objective edit
  that does not copy the network.
- Implement `DcPfSolution`, `AcPfSolution`, `DcOpfSolution`, `AcOpfSolution`,
  `McAcPfSolution`, `McAcOpfSolution`, and `AcScucSolution`. Each solution
  shares the immutable instance it solves and carries stable element
  identities.
- Require bus injections, bus voltages, and branch flows on `DcPfSolution` and
  `AcPfSolution`. Keep individual generator outputs optional unless uniquely
  determined or explicitly allocated.
- Remove the current public preparation tables and ambiguous public DC data
  objects. Equivalent contiguous arrays may remain private and cached.
- Add transformations from source instances to other calculation instances,
  with diagnostics for discarded data and assumptions.
- Encode DC power flow bus specifications and the standard AC power flow PQ, PV, reference, and
  isolated bus specifications explicitly. A PF instance contains these partial
  specifications, not a complete operating point. A PF solution contains the
  resulting operating point, termination, and residuals. Reject conflicting active voltage controllers
  during `AcPfInstance` construction until an explicit edit resolves them.
- Preserve zero impedance branches in networks and instances. Remove every
  default branch skip from finite matrix and problem construction paths. Add a
  checked, explicit `merge_zero_impedance_buses` transformation and require it
  to return mappings and diagnostics for removed branch behavior.

Issue coverage: the new public instances, objectives, and solutions trackers.

Foundation completion gate:

- all workspace crates compile with minimal and full feature sets;
- serde and schema snapshots are reviewed deliberately;
- instance clone tests prove that shared networks and source bytes are not
  duplicated;
- no public type exposes solver row arrays;
- an independent API review finds no remaining ambiguous 0.9 names.

### Wave 2: matrices, formats, and parser hardening

These areas followed the foundation because they consumed the settled module,
network, identity, and instance types.

#### Matrices

The DC matrix work implements the PowerModels incidence orientation,
branch susceptance formulas, bus susceptance matrix, branch susceptance matrix,
phase shift injection, and stable row mappings. Public values use PowerModels
signs. Internal positive factor weights use a name that describes their solver
role and convert sign while filling the caller's output buffer.

It keeps `DcPfInstance` matrix free. Separate matrix operations return `A`,
`B`, `Bf`, bus power injection, phase shift injection, and the reference
constrained linear system. An operating point update does not reconstruct the
network dependent matrices.

The AC Jacobian work implements one sparse `calc_power_flow_jacobian`
operation with polar and Cartesian coordinates. It returns the full physical
derivatives of active and reactive bus injections with respect to voltage
coordinates. The result agrees with MATPOWER `dSbus_dV` and the full form of
`makeJac`. Tests reconstruct PowerModels `calc_basic_jacobian_matrix` from the
physical derivative and bus types without putting its mixed solver variables in
the public matrix. Finite differences and an independent automatic
differentiation implementation check every derivative block. Sparse structure
is reused and numerical values update in place across operating points with
unchanged topology.

The multiconductor matrix work implements direct passive nodal admittance
and the augmented system for ideal equipment. It follows BMOPF and BMOPFTools
terminal ordering, units, and equations. It preserves conductor mappings,
merges exact unity connections, and never inserts arbitrary small impedances.

The sensitivity work factors once, reuses scratch buffers, changes dense
loop order for cache locality, and removes avoidable quadratic buffers.

Issue coverage: #232, #291, #294, #324, and #407.
Issue #400 closed when ABI 6 exposed its opaque `PioDcData` array owner. That
FFI owner is not a shared high level domain type.

#### Formats

The problem format work maps DOE GO Challenge 3 JSON to `AcScucInstance`, BMOPF JSON to
`McAcOpfInstance`, and DeepMind OPFData to `AcOpfSolution`. BMOPF
per terminal and per phase arrays remain exact. Regulator and general
transformer rows become typed data.

The PyPSA and Egret work implements the exact PyPSA CSV electrical profile as
`BalancedNetwork`, `TimeSeries<BalancedNetwork>`, or
`TimeSeries<OperatingPoint<BalancedNetwork>>` according to what varies. It
types the accepted snapshot-local columns and rejects or diagnoses
intertemporal calculation tables, non-electrical components, investment
periods, and stochastic data rather than silently reducing them. Retained source preserves exact same format output;
cross-format writes report every retained section the target drops. Tests
compare the profile against the current PyPSA main branch. Complete PyPSA and
NetCDF support waits for source neutral multi-carrier, multi-period, capacity
expansion, stochastic calculation, and result types. The same phase
implements Egret `system.time_keys` and `TimeSeries<BalancedNetwork>` when
every varying attribute belongs to the supported scalar network profile,
without cloning static tables for each time point.
PyPSA component classes outside the profile and Egret optimization fields
outside the accepted instances produce diagnostics and remain source
preserving.

The distribution format work states and enforces the static OpenDSS circuit
profile. Schedules, calculation instructions, and output requests outside that
profile remain source preserving and produce diagnostics. It supports
`TimeSeries<OperatingPoint<MulticonductorNetwork>>` at the dynamic boundary and
tests shared network ownership with QSTS shaped data from the official OpenDSS
semantics. It does not claim a QSTS solution merely by parsing a `.dss` script.

The GridFM work maps the current profile to
`ScenarioSet<BalancedNetwork>`, replaces independent repeated tables with shared
element identities and typed changes, preserves topology and parameter changes,
and reuses sparse structure only when valid. A solution profile uses a scenario
set of a solution type only when the source identifies the calculation and
supplies the required solution data.

The format fidelity work completes the remaining distribution and cost
conversion issues and verifies every advertised profile.

Issue coverage: #307, #360, #376, and #383, plus the new BMOPF, DOE GO Challenge 3,
DeepMind, PyPSA, Egret, and GridFM trackers.

#### Parser memory and hardening

The parser memory work removes duplicated JSON trees, cloned keys and matrices, and
owned token strings. Typed Serde decoding replaces generic JSON trees for known
fields. Format specific scanners borrow source ranges and parse numeric bytes
directly.

The binary and directory hardening work bounds PowerWorld binary and display searches, exercises
OpenDSS include containment and nesting, and adds malformed input allocation
limits.

Issue coverage: #274, #293, #338, and #339.

Wave 2 completion gate:

- all direct and converted numerical data agrees with independent tools;
- allocation gates improve or remain within an explicitly reviewed budget;
- large PyPSA projections, Egret sequences, and OpenDSS state sequences do not
  allocate one network per point;
- GridFM multi scenario parses share topology and identity data;
- fuzz and malformed input tests stay within byte, allocation, and recursion
  limits.

### Wave 3: `PioModule` and MCP

- Complete the `NetworkPackage` to `PioModule` rename across every language
  and command. The crate restructure landed with the foundation work; this
  wave finishes generic module records in `powerio-core` and dynamic and
  stored behavior in the `powerio` facade.
- Define the `.pio.json` schema separately from the Rust memory layout, with
  `schema: "powerio.module"`, `version: 1`, and the exact flat identifiers in
  `V1_ARCHITECTURE.md`. Decode that representation directly. Reject every
  other schema name or version and unknown fields or identifiers. Do not add
  per value versions or readers for beta documents.
- Implement only the common records fixed in the architecture: `producer`,
  `sources`, `source_map`, `diagnostics`, `history`, and namespaced
  `extensions`. History is structured and descriptive, not replayable. Source
  maps use relation kinds and zero or more checked source spans. Do not
  serialize validation runs, derived summaries, or runtime source ownership.
- Preserve nonfinite floats as `"Infinity"`, `"-Infinity"`, and `"NaN"` in
  typed float positions and reject `null`. Complete the DTO, generated JSON
  Schema, deterministic round trip fixture, reference validation, and binding
  test for every promoted `PioValue` before version 1 is frozen.
- Permit exactly one declared network, time series, scenario set, instance, or
  solution value.
- Stable element identities belong to the typed value. Do not add parallel operating
  point, time series, scenario, investment period, matrix, graph, `derived`,
  or `solutions` fields.
- Treat a solution as the primary value only when a source explicitly
  represents a solved calculation. Treat stored
  voltages and setpoints without a named solved calculation as operating point
  data.
- Apply typed state values without a generic JSON round trip or network clone.
- Expose value inspection, supported operation discovery, typed state
  selection, transformations, matrix data, diagnostics, and writing through
  MCP.
- Keep multiconductor to balanced transformation explicit.

Issue coverage: #261, #397, and #398, plus the new `.pio.json` tracker.

### Wave 4: ABI v6 and language bindings

- Use opaque owned handles for modules, values, networks, instances, solutions,
  matrices, numerical arrays, and errors. Give every handle type `retain` and
  `release`; `release(NULL)` is a no-op.
- Return independently owned child handles. Releasing a parent never
  invalidates a child.
- Make calculation result handles own their native arrays. Accessors return
  immutable spans valid until the result handle is released. Use caller fill
  buffers only for sign, unit, index, layout, or subset conversion.
- Return structured `PioError` handles rather than thread local state or a
  fixed character buffer. Catch panics at every `extern "C"` boundary.
- Allow concurrent immutable calls. Releasing the same raw handle concurrently
  with a call remains caller error. State these rules in `powerio.h`.
- Convert signs and units while filling the requested buffer, without a second
  temporary vector.
- Exercise all ABI ownership paths from C, Julia, and a small Go client.
- Keep Python on PyO3 while sharing the same Rust owners and semantics. Julia
  read only array wrappers retain the owner and the exact library that created
  it; `copy` returns an ordinary mutable Julia array.

Issue coverage: #399 and #400.

The Julia binding work keeps owner handles alive for every borrowed array or nested
network. Public DC equations and signs match PowerModels directly. Wrapper
constructors and property access do not serialize whole networks or create
temporary sign converted vectors.

Wave 4 completion gate:

- C smoke tests pass under address and undefined behavior sanitizers;
- Miri covers supported unsafe ownership helpers;
- Python tests pass with the minimal and full extras;
- Julia tests pass against local ABI v6 builds on every supported platform;
- the Go client parses, extracts, retains, and releases every handle type;
- header, Rust, Python, and Julia names agree.

### Wave 5: evaluation, audit, and release

The evaluation workspace moves cross tool cases and performance programs into
one nonpublished `evals/` workspace. Minimal fixtures remain beside ordinary
tests. Every case records source software revision, calculation settings,
identity mapping, tolerances, and expected diagnostics.

The evaluation matrix covers:

- original source parsed by PowerIO and an independent parser;
- PowerIO target output parsed by the target software;
- admittance, incidence, susceptance, limits, costs, time values, and scenarios;
- power flow and OPF solutions using PowerModels, Tellegen, ExaModelsPower,
  BMOPFTools, OpenDSS, Egret, PyPSA, and other suitable references;
- negative reactance, zero impedance, ideal switches, ideal transformers,
  islands, multiple reference buses, missing costs, malformed rows, long
  identifiers, truncated binary input, and deep include trees.

The audit phase records each correction with a reproduced failure or measured
result.

The documentation phase rewrites the mdBook, README files, Rust docs, C
header, Python docs, Julia docs, CLI help, schema descriptions, examples, and
release migration notes against the final terminology and equations. It removes
obsolete names instead of explaining two APIs that never shipped. It includes
short task paths for parsing, transformation, conversion, matrix construction,
instance construction, and solver handoff. Format tables use one name for each
format and state the supported profile and unsupported data explicitly.

Documentation completion requires:

- no unresolved architecture term or superseded public name in user facing
  text, tests, comments, headers, schema descriptions, or examples;
- the Rust, C, Python, and Julia descriptions of each public type and operation
  to agree;
- all equations, signs, orientations, dimensions, units, and element mappings
  to match executable compatibility tests;
- all documentation examples to execute;
- strict Rust doc, mdBook build and test, Python pdoc, Julia documentation,
  C example, link, and terminology checks to pass.

The release phase installs and imports every wheel, builds every C archive,
reruns the documentation checks, verifies schema and ABI versions, and prepares
the 1.0.0 release notes. The corresponding PowerIO.jl work prepares the
reviewed 1.0.0 release intent after its version and changelog are final. Its
intent names `v1.0.0` and the canonical digest of the complete reviewed Julia
tree. The PowerIO binding gate must pass against that tree before a release tag
can build assets. After publication, the Julia updater is authorized to commit
only `Artifacts.toml`; it cannot select a different version or rewrite release
claims.

Issue coverage: #325. Any residual fix retains the issue number of the behavior
it corrects.

## Historical issue coverage

| Issues | Area |
|---|---|
| #375, #377 | foundation |
| #196 | foundation repeated typed values |
| #232, #291, #294, #324, #407 | matrix |
| #307, #360, #376, #383 | formats |
| #274, #293, #338, #339 | parser memory and hardening |
| #261, #397, #398 | `PioModule` and MCP |
| #399, #400 | ABI v6 |
| #325 | release |

This table covers the 22 issues that were assigned to the 1.0 milestone before
the final audit. `V1_ISSUE_AUDIT.md` links the 16 new PowerIO implementation
trackers and the new PowerIO.jl ABI tracker created by that audit.

## Post 1.0 issues

The following open issues remain assigned after 1.0 unless new evidence changes
their scope: #14, #25, #27, #84, #91, #107, #111, #123, #150, #151, #152, and
#237. The 1.0 design must leave clean extension points for them, but the release
does not claim those features.

Before release, add compile and binding tests proving that a new stable format
name and a new nonexhaustive Rust enum variant do not change existing ordinal
or serialized meanings. Exercise a synthetic file and directory.
This is the compatibility gate for the later RAWX, DGS, IIDM, UCTE,
MG-RAVENS, CIM, and CGMES parsers.

## Historical validation commands

```bash
cargo fmt --all --check
cargo test
cargo test -p powerio-capi
bash scripts/ci-clippy.sh
```

Feature specific, Python, Julia, documentation, sanitizer, fuzz, and evaluation
commands supplemented this gate for the affected surface. Default workspace
tests alone did not establish completion.
