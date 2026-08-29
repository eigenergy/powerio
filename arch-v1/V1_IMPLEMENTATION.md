# PowerIO 1.0 implementation

Status: execution guide for the work defined by
[V1_ARCHITECTURE.md](V1_ARCHITECTURE.md),
[V1_ONTOLOGY.md](V1_ONTOLOGY.md),
[V1_TERMINOLOGY.md](V1_TERMINOLOGY.md), and
[V1_ISSUE_AUDIT.md](V1_ISSUE_AUDIT.md). Those documents define semantics. This
file defines dependency order, PR stacks, agent ownership, and completion
evidence. [V1_RATIONALE.md](V1_RATIONALE.md) records why the selected API won
over the alternatives.

The release is PowerIO 1.0.0 with C ABI v6. Do not ship an intermediate public
API under 0.10.

`PioModule<T>` is the accepted top level compiler type. Every successful parse
returns one. `.pio.json` is its versioned dynamic serialization, not a case
format or a direct dump of Rust memory.

## Release outcome

PowerIO 1.0 is complete when:

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
- Rust, C, Python, Julia, and MCP expose the same public meanings;
- Tellegen and the independent Julia tools consume PowerIO instances without a
  second public network model;
- every issue in the 1.0 milestone is closed by verified code or an explanatory
  scope correction;
- correctness, allocation, memory safety, fuzz, and release gates pass.

## Preimplementation audit complete

The source profile review, adversarial API audit, and `arch-v1/prototype/`
closed the product decisions. The one crate and multi-crate prototypes compile
an unconstrained `PioModule<T>`, flat `PioValue`, recoverable facade
free-function narrowing, immutable cheap to clone network handles, private shared temporal data,
`TimeSeries<OperatingPoint<MulticonductorNetwork>>`, one stored document
version, and owned path and named memory destinations. Public traits for memory representation,
`BalancedNetworkData`, per value schema versions, `powerio-pkg`, and a separate
`powerio-diag` leaf were rejected. Do not run another broad architecture interview or infer a
competing design from the current 0.9 code.

The GitHub execution trackers listed in `V1_ISSUE_AUDIT.md` were created on
2026-08-25. Every branch
below must name the issues it closes. Rewrite issue titles that still use names
removed from the 1.0 API.

## Multiagent rules

The coordinating agent owns dependency order, branch state, issue updates, and
final integration. Other agents receive bounded implementation or audit tasks.

- Never let two agents edit the same checkout.
- Give each writing agent an isolated git worktree and one named branch.
- A shared checkout is safe only for nonwriting audits.
- One agent owns a branch until its commits and validation notes are handed
  back to the coordinator.
- The coordinator runs `gh stack`, rebases dependent branches, updates PR text,
  and submits stacks.
- An implementation agent does not change a lower stack layer from an upper
  branch. The coordinator moves the fix to the owning branch and rebases the
  branches above it.
- Each PR gets an independent semantic review. Unsafe Rust, the C ABI, binary
  parsing, directory traversal, and borrowed buffers also get a memory safety
  review.
- Audit agents report concrete file and line evidence. They do not make broad
  cleanup edits on an implementation branch.

Parallel work is limited by real dependencies. Parser baselines, independent
reference calculations, fuzz cases, and post 1.0 issue review can run while the
foundation stack is being implemented. Bindings cannot stabilize before the
Rust API does.

## `gh stack` setup

PowerIO and PowerIO.jl are separate repositories, so GitHub represents them as
separate stacks. Cross repository dependency is recorded in PR bodies and
validation notes.

The local PowerIO checkout is still associated with a completed stack containing
PRs 343 through 346. Remove only that local association before creating the 1.0
stacks. PowerIO.jl `main` is not currently associated with a stack.

All commands must be noninteractive:

```bash
git config rerere.enabled true
git config remote.pushDefault origin
gh stack unstack --local

gh stack init --base main agent/v1-architecture-baseline
gh stack submit --auto
gh stack view --json
```

Use `gh stack submit --auto --open` only after the branch tests pass and the PR
is ready for review. After a lower branch changes, run
`gh stack rebase --upstack`. Routine synchronization uses
`gh stack sync --remote origin`. Never run `gh stack view` without `--json`.

Do not create every branch at once. Add each of the other three foundation
branches when work on that Wave 1 layer begins. A later stack can temporarily
target the top branch of an unmerged prerequisite stack when its PR body states
that dependency and the exact merge order. After the prerequisite merges
externally, rebase the later stack onto updated `main`.

## Execution order

### Optional maintenance release before the 1.0 cut

Do this only if the 0.9.1 release is wanted. It does not block 1.0 design work.

1. Rebuild PowerIO.jl #116 from `main` as
   `agent/v091-powerdata-finiteness`, adding the raw path finiteness, ownership,
   allocation, and tests salvaged from #118.
2. Cut PowerIO 0.9.1 only if there is a backward compatible Rust patch to
   release. Do not include PowerIO #387 or #401 because their public
   `Iterative` to `Sparse` rename is not patch compatible.
3. PowerIO.jl's reviewed release intent fixes the Julia version, matching Rust
   tag, changelog, and canonical source digest before the PowerIO tag is
   published. The artifact updater may change only `Artifacts.toml` and then
   dispatches registration for the exact resulting SHA. If there is no Rust
   patch, defer both; the current workflow has no Julia only 0.9.1 path.
4. Do not merge any current Rust stack branch before the tag.

After the maintenance decision, retain #387's sparse factorization, #401's
routing and checked CSC work, and the sensitivity allocation fixes from #405
on `agent/v1-sensitivities` in the later matrix stack. Rebuild that work after
its prerequisite matrix branches. #402 and later obsolete public API branches
are not ancestors.

### Wave 0: baseline and issue coverage

1. Inspect status in PowerIO, PowerIO.jl, Tellegen, Egret, PyPSA, PowerModels,
   PowerModelsDistribution, BMOPFTools, ExaModelsPower, and bmopf-report. Fetch
   current remote refs without changing checked out files. Run tests in clean
   worktrees or at recorded current commits when a checkout contains user work.
2. Record exact revisions used by evaluation adapters.
3. Run the current Rust, C, Python, Julia, documentation, and conversion suites.
4. Record parser allocation counts, allocated bytes, peak resident memory, and
   wall time on the small and large evaluation cases.
5. Record current sparse matrix allocations for DC incidence, PTDF, LODF, and
   multiconductor admittance construction.
6. Build an issue to branch table covering every 1.0 issue and every missing
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

### Wave 1: foundation stack

#### `agent/v1-module-diagnostics-source`

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
  `PioValue` variant and expose
  `powerio::try_into_typed::<T>(module)`. Reverse conversion uses
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

#### `agent/v1-state-data`

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

#### `agent/v1-instances-solutions`

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

### Wave 2: parallel Rust stacks

These stacks start after the foundation stack is ready. While the foundation
remains unmerged, they can temporarily target its top branch when every
affected PR states the cross stack dependency and exact merge order. Rebase
each stack onto updated `main` after the foundation merges externally. They can
run in separate worktrees.

#### Matrix stack

Stack order:

```text
agent/v1-dc-matrices
agent/v1-acpf-jacobians
agent/v1-multiconductor-matrices
agent/v1-sensitivities
```

`agent/v1-dc-matrices` implements the PowerModels incidence orientation,
branch susceptance formulas, bus susceptance matrix, branch susceptance matrix,
phase shift injection, and stable row mappings. Public values use PowerModels
signs. Internal positive factor weights use a name that describes their solver
role and convert sign while filling the caller's output buffer.

It keeps `DcPfInstance` matrix free. Separate matrix operations return `A`,
`B`, `Bf`, bus power injection, phase shift injection, and the reference
constrained linear system. An operating point update does not reconstruct the
network dependent matrices.

`agent/v1-acpf-jacobians` implements one sparse `calc_power_flow_jacobian`
operation with polar and Cartesian coordinates. It returns the full physical
derivatives of active and reactive bus injections with respect to voltage
coordinates. The result agrees with MATPOWER `dSbus_dV` and the full form of
`makeJac`. Tests reconstruct PowerModels `calc_basic_jacobian_matrix` from the
physical derivative and bus types without putting its mixed solver variables in
the public matrix. Finite differences and an independent automatic
differentiation implementation check every derivative block. Sparse structure
is reused and numerical values update in place across operating points with
unchanged topology.

`agent/v1-multiconductor-matrices` implements direct passive nodal admittance
and the augmented system for ideal equipment. It follows BMOPF and BMOPFTools
terminal ordering, units, and equations. It preserves conductor mappings,
merges exact unity connections, and never inserts arbitrary small impedances.

`agent/v1-sensitivities` factors once, reuses scratch buffers, changes dense
loop order for cache locality, and removes avoidable quadratic buffers.

Issue coverage: #232, #291, #294, #324, and #407.
Issue #400 closes later when the C surface exposes the completed DC data.

#### Format stack

Stack order:

```text
agent/v1-problem-formats
agent/v1-pypsa-egret
agent/v1-gridfm-scenarios
agent/v1-format-fidelity
```

`agent/v1-problem-formats` maps DOE GO Challenge 3 JSON to `AcScucInstance`, BMOPF JSON to
`McAcOpfInstance`, and DeepMind OPFData to `AcOpfSolution`. BMOPF
per terminal and per phase arrays remain exact. Regulator and general
transformer rows become typed data.

`agent/v1-pypsa-egret` implements the exact PyPSA CSV electrical profile as
`BalancedNetwork`, `TimeSeries<BalancedNetwork>`, or
`TimeSeries<OperatingPoint<BalancedNetwork>>` according to what varies. It
types the accepted snapshot-local columns and rejects or diagnoses
intertemporal calculation tables, non-electrical components, investment
periods, and stochastic data rather than silently reducing them. Retained source preserves exact same format output;
cross-format writes report every retained section the target drops. Tests
compare the profile against the current PyPSA main branch. Complete PyPSA and
NetCDF support waits for source neutral multi-carrier, multi-period, capacity
expansion, stochastic calculation, and result types. The same branch
implements Egret `system.time_keys` and `TimeSeries<BalancedNetwork>` when
every varying attribute belongs to the supported scalar network profile,
without cloning static tables for each time point.
PyPSA component classes outside the profile and Egret optimization fields
outside the accepted instances produce diagnostics and remain source
preserving.

The distribution format branch states and enforces the static OpenDSS circuit
profile. Schedules, calculation instructions, and output requests outside that
profile remain source preserving and produce diagnostics. The branch supports
`TimeSeries<OperatingPoint<MulticonductorNetwork>>` at the dynamic boundary and
tests shared network ownership with QSTS shaped data from the official OpenDSS
semantics. It does not claim a QSTS solution merely by parsing a `.dss` script.

`agent/v1-gridfm-scenarios` maps the current profile to
`ScenarioSet<BalancedNetwork>`, replaces independent repeated tables with shared
element identities and typed changes, preserves topology and parameter changes,
and reuses sparse structure only when valid. A solution profile uses a scenario
set of a solution type only when the source identifies the calculation and
supplies the required solution data.

`agent/v1-format-fidelity` completes the remaining distribution and cost
conversion issues and verifies every advertised profile.

Issue coverage: #307, #360, #376, and #383, plus the new BMOPF, DOE GO Challenge 3,
DeepMind, PyPSA, Egret, and GridFM trackers.

#### Parser memory and hardening stack

Stack order:

```text
agent/v1-parser-memory
agent/v1-binary-and-directory-hardening
```

The first branch removes duplicated JSON trees, cloned keys and matrices, and
owned token strings. Typed Serde decoding replaces generic JSON trees for known
fields. Format specific scanners borrow source ranges and parse numeric bytes
directly.

The second branch bounds PowerWorld binary and display searches, exercises
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

### Wave 3: `PioModule` and MCP stack

Stack order:

```text
agent/v1-pio-module
agent/v1-pio-state-selection
agent/v1-mcp-surface
```

- Complete the `NetworkPackage` to `PioModule` rename across every language
  and command. The crate restructure landed with the foundation stack; this
  wave finishes generic module records in `powerio-core` and dynamic and
  stored behavior in the `powerio` facade.
- Define a versioned `.pio.json` schema separately from the Rust memory layout,
  with `schema: "powerio.module"`, one integer document `version`, the exact
  flat identifiers in `V1_ARCHITECTURE.md`, and frozen one way upgrade fixtures
  for every released 0.9.x shape. Dispatch on the header before exact typed DTO
  decoding. Reject the pre 0.9 lineage and unknown current fields or
  identifiers. Reject nonempty legacy `study` with a directed instruction to
  materialize one selected revision using the 0.9 migration command. Do not add
  per value versions.
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
  or `solutions` fields. The literal legacy 0.9 `study` field is handled only
  by the upgrade reader.
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

### Wave 4: ABI v6 and language stacks

PowerIO stack order:

```text
agent/v1-abi6
agent/v1-python-api
agent/v1-mcp-release-api
```

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

PowerIO.jl is a separate formal stack:

```text
agent/v1-julia-abi6
agent/v1-julia-types
agent/v1-julia-matrices-and-solutions
agent/v1-julia-docs-evals
```

The Julia wrapper keeps owner handles alive for every borrowed array or nested
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

### Wave 5: evaluation, audit, and release stack

Stack order:

```text
agent/v1-evals-workspace
agent/v1-adversarial-audits
agent/v1-documentation
agent/v1-release
```

`agent/v1-evals-workspace` moves cross tool cases and performance programs into
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

`agent/v1-adversarial-audits` contains cross stack audit and evaluation fixes
whose natural owner is that branch. A finding in a lower layer is fixed on its
owning lower branch and rebased upward. The audit branch must not become a
miscellaneous refactor branch. Every edit links to a reproduced failure or
measured result.

`agent/v1-documentation` rewrites the mdBook, README files, Rust docs, C
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

`agent/v1-release` installs and imports every wheel, builds every C archive,
reruns the documentation checks, verifies schema and ABI versions, and prepares
the 1.0.0 release notes. The corresponding PowerIO.jl top branch prepares the
reviewed 1.0.0 release intent after its version and changelog are final. Its
intent names `v1.0.0` and the canonical digest of the complete reviewed Julia
tree. The PowerIO binding gate must pass against that tree before a release tag
can build assets. After publication, the Julia updater is authorized to commit
only `Artifacts.toml`; it cannot select a different version or rewrite release
claims.

Issue coverage: #325. Any residual fix retains the issue number of the behavior
it corrects.

## Current 1.0 issue ownership

| Issues | Owning stack |
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

## Required commands before each Rust stack submission

```bash
cargo fmt --all --check
cargo test
cargo test -p powerio-capi
bash scripts/ci-clippy.sh
```

Feature specific, Python, Julia, documentation, sanitizer, fuzz, and evaluation
commands are added by the owning branch and run before it becomes ready for
review. A branch is not complete because the default workspace tests pass.

Submit and inspect with:

```bash
gh stack submit --auto
gh stack view --json
```

## Fresh session goal

Implement PowerIO 1.0 from `AGENTS.md` and the complete audited baseline under
`arch-v1/`. Read the five normative V1 documents plus `V1_RATIONALE.md`, and run the prototype tests before
editing production code. Do not conduct another broad architecture interview
or restore public shapes from the 0.9 implementation. Treat the documented
unconstrained `PioModule<T>`, flat dynamic `PioValue` variants and
`PioValueKind` strings, facade-local `try_into_typed`, immutable cheap to clone network handles, private temporal
data sharing, one document schema version for `.pio.json`, instances, solutions,
multiconductor results, `Destination`, and crate ownership as settled. Do not
add public traits for memory representation, `BalancedNetworkData`, per value schema versions, or
restore `powerio-pkg`.

First inspect live issue, PR, branch, release, and CI state. If a 0.9.1 release
is wanted, make only the reduced maintenance patch described here and tag it
before any breaking Rust merge. Preserve current PR work through the recorded
salvage plan; do not merge obsolete solver row, incidence parts, or
`DcPowerFlowData` APIs. Rebuild the retained work as formal `gh stack` stacks
rooted at `main`, with PowerIO and PowerIO.jl in separate stacks.

Then start at Wave 0 and continue through the implementation waves. Use the
existing execution trackers, establish baselines, and implement each dependency
layer in an isolated worktree. Use focused nonwriting audits and bounded
implementation agents where work is independent. Reconcile source evidence
against MATPOWER, PowerModels, PowerModelsDistribution, BMOPF, DOE GO Challenge 3,
DeepMind OPFData, PyPSA, Egret, and GridFM; do not reopen product decisions that
those profiles already settle. Ask the user only if implementation uncovers a
genuine public product choice not answered by the audited documents or source
definitions.

Run the complete Rust, feature, C ABI, Python, Julia, documentation, numerical
compatibility, memory safety, allocation, fuzz, and release validation required
by the owning wave. Correct every DC branch flow test and document to include
`p_branch = -Bf * va + b .* shift`. Submit and independently review each stack
layer in dependency order. Continue until both repositories have clean formal
stacks ready to merge and every 1.0 completion gate has evidence, or report one
exact external or technical blocker that prevents further progress.
