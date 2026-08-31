# PowerIO 1.0 architecture

Status: historical 1.0 design baseline from 2026-08-25. It records the intended
shape that informed PowerIO 0.10, not the current implementation or API.

The companion [PowerIO 1.0 ontology](V1_ONTOLOGY.md) records the public types,
source formats, and allowed transformations as a knowledge graph.
The [PowerIO 1.0 terminology](V1_TERMINOLOGY.md) defines the words and names
used throughout the public API.
The [PowerIO 1.0 issue audit](V1_ISSUE_AUDIT.md) reconciles this design with the
GitHub milestones.

External definitions anchor this design:

- the DOE GO [Challenge 3 data format](https://data.openei.org/files/5997/Challenge3_Data_Format_20230124.pdf);
- the DOE GO [Challenge 3 mathematical specification](https://data.openei.org/files/5997/Challenge3_Problem_Formulation_20240122.pdf);
- [PowerModels](https://lanl-ansi.github.io/PowerModels.jl/stable/) terminology and matrix semantics where PowerIO exposes compatible data;
- OpenDSS [load shape](https://opendss.epri.com/LoadShape.html) and [solution option](https://opendss.epri.com/Options.html) definitions;
- PyPSA [time and scenario indexing](https://docs.pypsa.org/stable/api/networks/indexing/), [stochastic optimization](https://docs.pypsa.org/latest/examples/stochastic-optimization/), and [pathway planning](https://docs.pypsa.org/stable/user-guide/optimization/pathway-planning/).

## Purpose

PowerIO parses power system sources into typed data, transforms between those
types, writes supported formats, and reports structured diagnostics. It does not
solve power flow or optimization problems.

Conversion remains a primary operation. A one call file conversion composes
parse, any required typed transformation, and write operations, then returns
their combined diagnostics. Users do not have to manage the intermediate typed
data to convert one supported format into another.

PowerIO should make useful lossy transformations possible. When a richer model
is transformed into a simpler model, PowerIO uses documented defaults where the
result remains meaningful and reports every assumption and loss. A
transformation that lacks data required by the target asks for explicit inputs
or fails with structured diagnostics.

## Compiler design references

PowerIO follows these narrow lessons from LLVM and MLIR:

- one top level module holds typed intermediate data;
- the module in memory and its serialized forms are separate representations
  of the same data;
- successful parsing and every explicit transformation end with verification;
- serialization has its own versioning and upgrade rules;
- target conversion has a read only legality preflight before rewriting, so a
  writer can report every unsupported field before strict or explicit lossy
  output;
- internal capability interfaces let writers, verifiers, matrices, and
  transformations consume concrete types without global `PioValue` switches;
- symbolic element identity survives lowering while row numbers remain private
  implementation details;
- analyses are computed on demand and invalidated unless a transformation
  declares that it preserves identity, topology, parameters, or state;
- owning handles and borrowed views are distinct APIs.

PowerIO does not copy MLIR's generic operation tree, dialect registry, nested
regions, or global context. Its existing Rust types already express the power
system data more directly. A `PioContext` should be introduced only if measured
string interning, shared allocation, or extension registration needs justify
its lifetime and concurrency cost.

References: LLVM [library layering](https://llvm.org/docs/CodingStandards.html#library-layering),
[language reference](https://llvm.org/docs/LangRef.html), and
[`Module` class](https://llvm.org/doxygen/classllvm_1_1Module.html),
[`SourceMgr`](https://llvm.org/doxygen/classllvm_1_1SourceMgr.html), and the
[new pass manager](https://llvm.org/docs/NewPassManager.html); MLIR
[`builtin.module`](https://mlir.llvm.org/docs/Dialects/Builtin/#builtinmodule-moduleop),
[diagnostics](https://mlir.llvm.org/docs/Diagnostics/),
[transformation infrastructure](https://mlir.llvm.org/docs/PassManagement/),
[dialect conversion](https://mlir.llvm.org/docs/DialectConversion/),
[interfaces](https://mlir.llvm.org/docs/Interfaces/),
[symbols](https://mlir.llvm.org/docs/SymbolsAndSymbolTables/), and
[bytecode versioning](https://mlir.llvm.org/docs/BytecodeFormat/).

## Source semantic completeness

A parser documents the part of a source format that it supports. Within that
profile, every defined field becomes typed PowerIO data or produces a
diagnostic saying why it cannot be represented. It does not survive only as raw
JSON, an untyped extra, or retained text.

PowerIO does not claim complete support for an upstream format when it only
supports one profile of that format. Sections outside the supported profile may
remain in `Source` for exact same format writing, but parsing reports
that PowerIO did not interpret them. Adding another profile expands the typed
ontology instead of silently changing the meaning of the existing profile.

Within the supported profile, the parsed value is the richest PowerIO type that
the source declares. BMOPF JSON produces
`McAcOpfInstance`, which contains its `MulticonductorNetwork`. A DOE GO Challenge
3 JSON source produces `AcScucInstance`, which contains its
`BalancedNetwork`. Extracting only the network from either instance is a
separate transformation that reports the discarded problem data.

A format does not become a problem instance merely because it contains enough
parameters to construct one. A reusable network format can contain equipment
limits, cost curves, and a stored operating state that several calculations can
use. A format that defines a particular calculation, objective, time horizon,
or reliability requirements maps directly to the corresponding instance.

The 1.0 profiles already settled are narrower than the whole upstream formats:

- Surge support covers the electrical network data PowerIO already parses and
  writes. Surge market and control sections are after 1.0.
- Egret support covers the scalar network `ModelData` profile. When
  `system.time_keys` is present and every varying attribute belongs to that
  scalar profile, it produces `TimeSeries<BalancedNetwork>` by applying
  Egret's own scalar snapshot rule. Static tables are shared between entries.
  Unit commitment, reserves, contingencies, and security constraints remain
  outside the 1.0 profile.
- PyPSA 1.0 support is the documented CSV electrical profile. One snapshot
  produces `BalancedNetwork`. Supported snapshot-local input series produce
  `TimeSeries<BalancedNetwork>`. A fixed network with only complete electrical
  state output varying produces
  `TimeSeries<OperatingPoint<BalancedNetwork>>`; input parameter changes plus
  state output remain `TimeSeries<BalancedNetwork>`. Non-electrical components,
  intertemporal calculation data, investment periods, and stochastic
  optimization are outside that profile. They remain retained for exact same
  format writing and are reported before a cross-format projection. PowerIO does not introduce a source
  specific `PyPsaModel`. Full support requires source neutral multi-carrier,
  multi-period, capacity expansion, stochastic calculation, and result types.
  PyPSA NetCDF is not claimed as a complete 1.0 format.
- GridFM support covers the static bus, generator, branch, and Y-bus Parquet
  profile. It preserves every sample as `ScenarioSet<BalancedNetwork>` and
  reuses one base element identity map. The newer dynamic Zarr trajectories,
  perturbation key, machine and controller trajectories, and runtime metadata
  are outside the 1.0 profile.

The accepted classifications are: DeepMind OPFData JSON produces
`AcOpfSolution` with termination `NotReported`, the source's stated solution
claim, and residuals computed by PowerIO. Its source supplied initial `pg`,
`qg`, and `vg` stay in solve metadata; omitting any of them emits a diagnostic.
MATPOWER, single network PowerModels
JSON, pandapower JSON, PSS/E RAW, PowerWorld AUX and PWB, PSLF EPC, static
Egret, and the Surge electrical profile produce `BalancedNetwork`;
PowerWorld PWD belongs to the display API and produces no module value;
OpenDSS and PMD engineering JSON produce `MulticonductorNetwork`; DOE GO
Challenge 3 input produces `AcScucInstance`; and current GridFM Parquet
produces `ScenarioSet<BalancedNetwork>`. PowerModels multinetwork, PMD
mathematical or result data, and DOE solution JSON require separately named
profiles rather than inheriting a classification from a related format.

OpenDSS load shapes, solution modes, controls, monitors, meters, sampled QSTS
states, and dynamic states are distinct source concepts. A `.dss` script parse
does not claim that a solve occurred. The static 1.0 profile returns
`MulticonductorNetwork`. Load shapes, solve instructions, monitor and meter
requests, and other calculation constructs outside that static profile remain in the
retained source for exact writing and receive an uninterpreted profile
diagnostic. `TimeSeries<OperatingPoint<MulticonductorNetwork>>` is a built in
dynamic value for complete sampled QSTS states supplied by another source or
simulator. Load shapes and solution instructions are not mislabeled as that
value. A named QSTS instance and solution are required before PowerIO claims
QSTS interchange; dynamic simulation is a separate later profile.

Format defined defaults are part of parsing. A PowerIO choice needed to
construct another ontology type belongs to an explicit transformation, is
recorded in diagnostics, and never changes the meaning attributed to the
source. PowerIO does not invent a missing objective, schedule, contingency,
control mode, or solution so that parsing succeeds.

Retained source data supports byte exact writing and fields outside the
documented profile. It is not a substitute for typing fields inside that
profile. Adding support for another field means adding it to the appropriate
ontology type and conversion diagnostics.

## Public ontology

```text
Balanced network formats ──────────► BalancedNetwork
                                      ├── DcPfInstance
                                      ├── AcPfInstance
                                      ├── DcOpfInstance
                                      ├── AcOpfInstance
                                      ├── matrix data
                                      └── graph data

Multiconductor network formats ────► MulticonductorNetwork
                                      ├── graph data
                                      ├── multiconductor matrix data [required for 1.0]
                                      ├── McAcPfInstance [required for 1.0]
                                      ├── McAcOpfInstance [required for 1.0]
                                      └── BalancedNetwork
                                          through a documented lossy
                                          transformation

BMOPF JSON ─────────────────────► McAcOpfInstance
                                      └── network: MulticonductorNetwork

DOE GO Challenge 3 JSON ───────────► AcScucInstance
                                      └── network: BalancedNetwork

Built in dynamic value ──► PioModule<PioValue> ──► .pio.json
Application typed value ─► PioModule<T> [Rust only until promoted]
```

### Network types

`BalancedNetwork` is the format neutral balanced positive sequence transmission
network. MATPOWER, PSS/E, PSLF, PowerModels JSON, Surge JSON, and the other
balanced network formats parse into it.

`MulticonductorNetwork` is the conductor level distribution model. OpenDSS,
PMD JSON, and other network data formats parse into it. BMOPF JSON also contains
optimization input and is addressed separately below.

The two network types remain distinct. A documented multiconductor to balanced
transformation is a first class PowerIO capability.

Neither type is declared a subtype or superset of the other. A
`MulticonductorNetwork` carries conductor identities, terminal connections,
coupled impedance matrices, grounding, and distribution equipment. A
`BalancedNetwork` carries one balanced positive sequence representation plus
transmission semantics. Conductor resolution is more detailed along one
dimension; it does not make the current multiconductor network a universal
representation of every power system source.

PowerIO can still construct a conductor resolved equivalent from a balanced
network. That operation must state assumptions about phase replication,
neutral and ground conductors, transformer connections, and voltage bases. It
is an explicit transformation with diagnostics, not a type cast or an argument
for combining both network types into one object. It is additive work after
1.0 and does not block the stable conversion graph.

`MulticonductorNetwork` already produces direct graph data. Direct
multiconductor matrix data and distribution problem instances also belong on
this branch of the ontology. They do not require conversion to
`BalancedNetwork`.

PowerIO 1.0 must build multiconductor admittance matrix data directly from a
`MulticonductorNetwork`. This is a missing PowerIO feature, not a BMOPFTools
operation that users are expected to compose. `powerio-matrix` owns matrix data
for both network types. The C and language bindings expose the same direct path
without serializing through BMOPF JSON or allocating a second network model.

The BMOPF mathematical specification defines the terminal, conductor,
grounding, element, and SI unit semantics. BMOPFTools provides an independent
executable reference for system matrix assembly and validation against
OpenDSS. PowerIO owns its implementation.

The direct multiconductor matrix and the matrix obtained after conversion to a
balanced equivalent are different results. PowerIO supports both routes and
reports the assumptions and losses on the balanced route.

### Problem instance types

The public problem instance types are:

- `DcPfInstance`
- `AcPfInstance`
- `DcOpfInstance`
- `AcOpfInstance`
- `McAcPfInstance`
- `McAcOpfInstance`
- `AcScucInstance`

Every instance contains or shares its reusable electrical network instead of
duplicating that network as solver preparation arrays:

```text
DcPfInstance  ── network: BalancedNetwork
AcPfInstance  ── network: BalancedNetwork
DcOpfInstance ── network: BalancedNetwork
AcOpfInstance ── network: BalancedNetwork
McAcPfInstance ─ network: MulticonductorNetwork
McAcOpfInstance ─ network: MulticonductorNetwork
AcScucInstance ─ network: BalancedNetwork
```

Instance specific data is stored alongside the network and refers to network
elements by stable identity. `BalancedNetwork` and `MulticonductorNetwork`
become cheap owning handles over private immutable shared tables.
Cloning a network handle does not clone its tables. Sibling instance
transformations can therefore borrow a module value, clone its handle, and
share the same allocation. Solutions hold a shared immutable instance owner.
`instance.clone()` does not duplicate network tables and `solution.clone()`
does not duplicate its instance or network.

Rust instance fields are private. Each instance exposes a borrowed network
accessor such as `network(&self) -> &BalancedNetwork` or
`network(&self) -> &MulticonductorNetwork`. The accessor does not allocate or
copy. Keeping the shared tables private lets PowerIO move between direct ownership
and shared ownership without changing the public API. The C API returns a
retained network handle rather than a pointer into an instance. Julia and
Python keep that owner alive while exposing the network.

The current `DcOpfInstance` and `AcOpfInstance` expose structures of parallel
vectors: dense bus numbers, branch endpoint columns, admittance columns,
generator cost columns, source row maps, and omitted row lists. These arrays
exist to assemble matrices and solver equations quickly. They duplicate data
from the network, allocate during construction, and expose a solver oriented
indexing scheme as public PowerIO data.

Those arrays are not 1.0 public instance fields. A solver or matrix builder may
create the same contiguous arrays as private preparation data and cache them
for repeated calculations. The public instance retains the typed network,
calculation inputs not already stored on that network, and the settings needed
to define the requested calculation.

An OPF instance is more than a network wrapper. Physical limit values remain on
the network so power flow and other calculations can reuse them. The OPF
instance identifies the active constraints by stable element identity, contains
the typed objective, and carries calculation data such as commitment,
contingencies, or time requirements when that problem needs them. It references
network limits instead of copying their numerical arrays.

`DcPfInstance`, `AcPfInstance`, and `McAcPfInstance` contain partial boundary
specifications. They do not contain a required complete operating point: the
unknown voltages, injections, and flows are what the calculation solves. A
complete `OperatingPoint<N>` can be supplied as an optional solver initial
state. OPF instances follow the same initialization rule.

Each OPF instance contains its objective as typed objective terms. A solver
does not replace that objective with a generation cost selector. Solver options
choose equations, algorithms, tolerances, and initialization; changing the
mathematical objective constructs a different instance. The exact objective
term enums are nonexhaustive typed references to costs and penalties stored on
the network or calculation record. Active constraint selections likewise use
stable element identities rather than copied numerical bounds.

The calculation specific semantic fields are:

- `DcPfInstance`: net active power at nonreference buses, voltage angle at
  reference buses, isolated buses, and the selected DC branch approximation;
- `AcPfInstance`: one `AcBusSpecification` per bus:
  `Pq { p, q }`, `Pv { p, vm }`, `Reference { vm, va }`, or `Isolated`;
- `DcOpfInstance`: typed objective terms, active constraints, the selected DC
  approximation, and reference conditions;
- `AcOpfInstance`: typed objective terms and active generator capability,
  voltage, thermal, angle, and equipment constraints;
- `McAcPfInstance`: prescribed terminal complex powers or currents, prescribed
  source terminal complex voltages, isolated terminals, load voltage models,
  and active equipment control modes;
- `McAcOpfInstance`: exact per phase BMOPF objective references and active
  terminal voltage, conductor current or apparent power, and per phase
  generator constraints;
- `AcScucInstance`: the complete DOE GO Challenge 3 input categories listed
  below.

Tellegen's differentiability regularization is therefore an explicit typed
objective term. PowerIO provides an efficient objective edit that creates the
requested instance without copying its shared network. A solver never adds the
term silently. The derived instance and a `.pio.json` history can state the
term and its weight exactly.

`AcScucInstance` follows the DOE GO Challenge 3 mathematical definition and data
format. It contains the balanced electrical network plus:

- time points and interval durations;
- initial equipment and device states;
- time varying active and reactive bounds, availability, and status limits;
- energy price blocks and on, start, shutdown, ramp, and reserve costs and
  limits;
- reactive capability modes;
- active and reactive reserve zones and their memberships;
- energy windows;
- contingencies, emergency ratings, DC lines, and transformer controls;
- violation costs.

Parsing DOE GO Challenge 3 JSON constructs an
`AcScucInstance` directly. Its `network` is the same `BalancedNetwork` exposed
to callers. Asking only for that network discards scheduling, reserve, and
contingency data and reports those omissions.

This primary type does not restrict the calculations a caller can request.
`AcScucInstance::network()` exposes the balanced network by reference with no
allocation. PowerIO can construct `AcPfInstance`, `DcPfInstance`,
`AcOpfInstance`, or `DcOpfInstance` from the source instance when the required
inputs are present, returning diagnostics for assumptions and discarded data.
BMOPF follows the same rule. Parsing preserves the complete declared problem;
later transformations select a different calculation.

`ScopfInstance` is too ambiguous and is not a 1.0 public type. A SOC relaxation
uses the data in `AcOpfInstance`; it selects different equations rather than a
separate input instance.

### Solutions, operating points, and source collections

PowerIO owns power flow, optimal power flow, and unit commitment solution data.
A solution uses stable network element identities and
contains values, termination information, objective values where applicable,
and numerical residuals. It does not contain a solver's variable ordering,
factorization, cache, or internal status objects.

Each solution contains or shares the instance it solves, just as each instance
contains or shares its network. The instance stays immutable and can have zero,
one, or several solutions from different equations, solvers, settings, or
initial points. Putting an optional solution on the instance would instead mix
input with mutable result state and make several results awkward.

At run time, several solutions share one instance through shared ownership, so
neither the instance nor its network is copied. In a `PioModule`, a solution
value writes its instance once together with the result. DeepMind OPFData JSON
therefore parses to `AcOpfSolution`, which exposes its `AcOpfInstance`.

A solution is the module's primary value only when the source explicitly
represents a solved calculation. DeepMind OPFData does. A MATPOWER case or
PyPSA snapshot that stores voltages and generator setpoints does not by itself
claim that a named calculation converged, so it remains network data with an
operating point. A solver returns a typed solution directly. That solution can
be serialized in its own module when persistence is requested; every module
does not gain a generic list of solver results.

Every solution records the shared immutable instance, formulation identity,
producer or solver identity, termination and solution claim, numerical
residuals, and values keyed by stable element identity. The accepted solution
type names are `DcPfSolution`, `AcPfSolution`,
`DcOpfSolution`, `AcOpfSolution`, `McAcPfSolution`, `McAcOpfSolution`, and
`AcScucSolution`.

- `DcPfSolution` has bus angles and injections and branch terminal active
  flows.
- `AcPfSolution` has complex bus voltages, active and reactive bus injections,
  and terminal branch flows.
- `DcOpfSolution` adds generator active dispatch and objective to the DC power
  flow results.
- `AcOpfSolution` adds generator active and reactive dispatch and objective to
  the AC power flow results.
- `McAcPfSolution` has terminal complex voltages, terminal currents and powers,
  and source injections.
- `McAcOpfSolution` adds per phase generator dispatch and objective to the
  multiconductor power flow results.
- `AcScucSolution` preserves the DOE output fields: bus `vm` and `va`; shunt
  `step`; device `on_status`, `p_on`, `q`, regulation, synchronous,
  nonsynchronous, ramping, and reactive reserves; AC line `on_status`;
  transformer `tm`, `ta`, and `on_status`; and DC line `pdc_fr`, `qdc_fr`, and
  `qdc_to`.

Individual generator outputs remain optional on power flow solutions unless
the instance determines them uniquely or the source records an explicit
allocation. Duals remain an additive post 1.0 result because none of the
required source profiles needs them. Derived flows and objective components
not present in an input profile are verifier results, not fabricated source
fields.

GridFM snapshots are not automatically called solutions. A snapshot with only
stored network state is an operating point. It is a solution only when the
source states which calculation was solved and supplies the data needed to
interpret and validate that result.

An external data collection is not another electrical model. It can contain
typed PowerIO values. The collection may contain independent networks,
instances, or solutions, so one generic public type must not hide what each
entry is.

Time and scenarios are independent dimensions. `TimeSeries<T>` is ordered by
`TimePoint`. A time point stores the exact nonempty source label and an optional
nonnegative `std::time::Duration`; it does not parse calendar or time zone
meaning into an underspecified string field. `TimeSeries::get(i)` returns the
time point and `&T`, while `value(i)`, `time_point(i)`, and paired iteration are
allocation free. `ScenarioSet<T>` contains named alternatives with no implied
order. Alternatives that each vary through time use
`ScenarioSet<TimeSeries<T>>`. A scenario set represents independent
alternatives or realized samples. Shared first stage decisions,
nonanticipativity, recourse, and risk measures belong to a named stochastic
calculation instance. `ScenarioId` is a case sensitive nonempty string and is
never normalized. A set can be empty. It supplies finite nonnegative
probabilities for every entry or none; when present, their sum must be within
`1e-12` of one. PyPSA investment periods remain source data outside the
1.0 electrical profile. An external collection whose entries share neither
time nor scenario semantics is not forced into either container.

An `OperatingPoint<N>` contains the instantaneous electrical state and the
actual algebraic equipment settings needed to interpret it. Switch and
in-service status, transformer or regulator tap position, phase shift, and
capacitor step belong to the operating point when they change the
instantaneous equations. Schedules, bounds, costs, controller queues, storage
history, commitment history, and solve status do not. A complete simulator
checkpoint can contain an operating point plus equipment and control state.

`T` states what changes. Electrical state over time is
`TimeSeries<OperatingPoint<BalancedNetwork>>`. If branch parameters, element
inventory, terminal connectivity, limits, or costs change, the value is not an
operating point; it is a time series or scenario set of the network or
calculation type that owns those fields. A deterministic series is not wrapped
in `ScenarioSet<T>`. DOE GO
Challenge 3 contingencies remain contingencies inside `AcScucInstance`.

The public generic containers have private fields and ordinary borrowed access.
`TimeSeries<T>::get` returns `(&TimePoint, &T)` and scenario lookup returns
`&Scenario<T>`. A concrete
type such as `OperatingPoint<N>` can be a small owning handle into shared
numerical columns and a cheap to clone immutable network handle. Retaining a point
then remains valid after its parent collection is dropped without allocating a
network. The memory layout and any internal traits that support it remain private, so
column, sparse change, and table sharing strategies can change without a
public API break.

A power flow solution owns or shares its immutable instance and contains the
resulting complete operating point, termination, and residuals. Time varying
limits, costs, reserve requirements, and commitments belong to a
calculation type that defines their meaning and are not operating point fields.
Ramping, startup, storage balance, energy windows, and other intertemporal
relations require one named multi-period instance; a time series of independent
single-period instances does not preserve them.

Egret stores one ordered list of time labels. GridFM stores scenarios. PyPSA
has independent snapshot, stochastic scenario, and investment period axes.
DeepMind OPFData collections contain solved AC OPF entries that may have
different networks. The 1.0 representation must keep these distinctions while
sharing a base network and typed updates whenever element identities are
stable. These axes do not become common `PioModule` fields. Complete PyPSA
support waits for the source neutral representations those axes require.

### MATPOWER and PowerModels data

MATPOWER does support AC optimal power flow. Its case format carries bus
voltage bounds, generator active and reactive bounds, branch ratings and angle
bounds, and optional generator costs. `runopf` solves AC OPF by default.

That does not make every MATPOWER case an `AcOpfInstance`. The same case is
accepted by AC power flow, DC power flow, AC OPF, and DC OPF. The chosen
calculation and AC or DC equations are caller inputs, not declarations in the
case. PowerModels follows the same pattern: `parse_file` returns network data,
then `solve_pf` or `solve_opf` combines that data with an equation type.

The accepted rule is therefore:

- automatic MATPOWER and single network PowerModels JSON parsing returns
  `BalancedNetwork`;
- equipment ratings, capability bounds, stored state, and generator cost curves
  remain reusable source data on that network;
- constructing an OPF instance selects the enforced bounds and builds an
  explicit objective from the available cost data;
- generalized MATPOWER optimization fields, PowerModels multinetwork data, and
  embedded results require separately documented profiles rather than being
  hidden on `BalancedNetwork`.

This keeps one parsed source usable for several calculations without duplicating
its arrays. The objective itself remains instance data: constructing
`AcOpfInstance` or `DcOpfInstance` selects and references the relevant cost
curves and enforced limits.

### BMOPF JSON

BMOPF JSON parses as `McAcOpfInstance` in 1.0. The specification defines a
multiconductor optimization problem and
the JSON carries voltage and current limits, generator bounds, and per phase
generation costs in addition to the electrical network.

The current PowerIO parser returns `MulticonductorNetwork` and stores much of
that optimization input directly on buses, lines, switches, and generators.
That is convenient for conversion but blurs the boundary established above for
balanced network data and problem instances.

It also loses valid BMOPF distinctions: the schema permits voltage bounds per
bus terminal and generation costs per phase, while the current
`MulticonductorNetwork` stores one scalar bus bound and one scalar generator
cost. The parser collapses nonuniform arrays and reports a diagnostic. The 1.0
primary BMOPF parse result must preserve those arrays exactly. A scalar collapse
is allowed only in an explicitly lossy transformation to a target that cannot
represent them.

The accepted 1.0 shape is analogous to DOE GO Challenge 3:

```text
BMOPF JSON -> McAcOpfInstance
               -> network: MulticonductorNetwork
```

`McAcOpfInstance` is the confirmed name. `Mc` follows
PowerModelsDistribution's `solve_mc_opf` terminology, while `AcOpf` states the
underlying electrical problem. AC polar, AC Cartesian, current voltage, SOC,
and SDP describe equation representations or relaxations selected by the
solver; they do not create different input instances.

BMOPF voltage bounds remain per terminal arrays and generator costs remain per
phase arrays on `MulticonductorNetwork`. The other conductor resolved limits
also retain their source dimensions. `McAcOpfInstance` shares that network and
adds only problem input that does not belong on the reusable electrical model.
It does not collapse or copy these arrays.

## Parsing

`PioModule<T>` is the universal top level PowerIO compiler type. There is no
second `Parsed<T>` wrapper.

`PioModule<T>` places no marker bound on `T`. Application code can use
`PioModule<MyValue>` without asking PowerIO to register the type. `PioValue` is
the flat dynamic boundary used when parsing, stored JSON, or a language binding
must discover which built in value is present. It does not define every value
that a typed Rust module can contain. There is no public `ModuleValue`,
`ValueTypeId`, sealing module, trait for memory representation, or public auxiliary network data type.

Only concrete types produced by a supported parser, promised by `.pio.json`,
or required for binding discovery enter `PioValue`. Adding a generic type
combination to Rust does not settle it as a stored or binding API. Promotion
requires an enum variant, a stable `PioValueKind` string, an exact stored DTO, and
binding tests.

```rust
pub struct PioModule<T> {
    value: T,
    // Private common records.
}

#[non_exhaustive]
pub enum PioValue {
    BalancedNetwork(BalancedNetwork),
    MulticonductorNetwork(MulticonductorNetwork),
    BalancedNetworkTimeSeries(TimeSeries<BalancedNetwork>),
    BalancedOperatingPointTimeSeries(
        TimeSeries<OperatingPoint<BalancedNetwork>>,
    ),
    MulticonductorOperatingPointTimeSeries(
        TimeSeries<OperatingPoint<MulticonductorNetwork>>,
    ),
    BalancedNetworkScenarioSet(ScenarioSet<BalancedNetwork>),
    DcPfInstance(DcPfInstance),
    AcPfInstance(AcPfInstance),
    DcOpfInstance(DcOpfInstance),
    AcOpfInstance(AcOpfInstance),
    McAcPfInstance(McAcPfInstance),
    McAcOpfInstance(McAcOpfInstance),
    AcScucInstance(AcScucInstance),
    DcPfSolution(DcPfSolution),
    AcPfSolution(AcPfSolution),
    DcOpfSolution(DcOpfSolution),
    AcOpfSolution(AcOpfSolution),
    McAcPfSolution(McAcPfSolution),
    McAcOpfSolution(McAcOpfSolution),
    AcScucSolution(AcScucSolution),
}
```

`PioValueKind` is a nonexhaustive enum with one case per variant. Its
`as_str()` method returns these stable 1.0 identifiers:

```text
balanced_network
multiconductor_network
balanced_network_time_series
balanced_operating_point_time_series
multiconductor_operating_point_time_series
balanced_network_scenario_set
dc_pf_instance                 dc_pf_solution
ac_pf_instance                 ac_pf_solution
dc_opf_instance                dc_opf_solution
ac_opf_instance                ac_opf_solution
mc_ac_pf_instance              mc_ac_pf_solution
mc_ac_opf_instance             mc_ac_opf_solution
ac_scuc_instance               ac_scuc_solution
```

`value() -> &T` borrows the typed value and `into_value() -> T` moves it out.
Matrix builders and solvers use those typed values directly. They do not parse
`.pio.json` or match `PioValue` when `T` is already known.

The module exposes borrowed access to its typed value and retained source. The
1.0 schema decision fixes the remaining common record accessors. Calculation
state and repeated problem data belong to their typed value rather than
parallel module fields.

The Rust API has one parse operation:

```rust
parse(source: Source) -> Result<PioModule<PioValue>, Error>
```

The source design follows LLVM's source manager model. `Source` is an opaque
owner or provider of named immutable byte buffers. `Source::open(path)`
acquires input from a file or directory. `Source::from_bytes(name, bytes)`
accepts memory input and requires a name for diagnostics and format detection.
A file can be memory mapped and a directory parser can request its files as
needed; the public type does not require eager `Vec<u8>` copies. This separates
source acquisition from parsing and avoids an ambiguous `parse("...")` API
where a string could mean a path or source text.

`Source` can also carry one declared format. Without a declaration, `parse`
detects the format, including structural detection for bare JSON. With a
declaration, `Source::open(path)?.with_format(format)` selects the parser,
following Clang's `-x` input override. The selected parser validates the input;
malformed content is a `Parse` error. The declaration belongs to the input, so
`parse` keeps one signature. Because `Source` lives in `powerio-core`, the
declaration is an open `FormatId`, not an enum imported from either network
crate. A format ID is ASCII lower case, starts with a letter, and contains only
letters, digits, and single hyphens between nonempty segments. C, Python,
Julia, stored JSON, and MCP use the same spelling.

All external encodings reach their parsers as bytes, including text formats.
`Source::from_bytes` takes owned or shared immutable bytes because the module
can retain it for source locations and exact writing. A temporary borrowed
slice would either require a copy or impose its lifetime on the module.

The typed output depends on the source:

- MATPOWER and other balanced network formats produce `BalancedNetwork`;
- network only distribution formats such as OpenDSS and PowerModelsDistribution
  JSON produce `MulticonductorNetwork`;
- BMOPF JSON currently produces `MulticonductorNetwork`; its 1.0 output will be
  `McAcOpfInstance`;
- DOE GO Challenge 3 JSON produces `AcScucInstance`.

A MATPOWER parse has no `instance` field. The earlier suggestion that every
parse result would expose `parsed.instance` was wrong.

The automatic parser returns the nonexhaustive `PioValue` enum. A Rust caller
that expects one concrete result moves the module into that type:

```rust
let module = parse(Source::open(path)?)?;
let module: PioModule<BalancedNetwork> = powerio::try_into_typed(module)?;
```

The conversion moves the enum value and module fields; it does not copy the
network or parse a second time. A mismatched expectation is the recoverable
`ValueKindMismatch` conversion error, not a parse or validation `Error`.
There is no `parse_as` operation: selecting a concrete enum variant after a
parse is a checked Rust conversion, while constructing a different calculation
instance is a PowerIO lowering.

The facade exposes `powerio::try_into_typed::<T>(module)`. Its sealed
`FromPioValue` trait connects each built in concrete type to one enum variant
and kind. The trait performs conversion; it is not a marker and does not bound
`PioModule<T>`. Standard module-level `TryFrom` cannot be implemented across
the crate split. Successful narrowing moves the value, source owner, and common
records without allocation. On mismatch, `ValueKindMismatch` owns the original
dynamic module, reports the expected and actual `PioValueKind`, and returns the
module through `into_module()`.

There is no stable public list of `parse_balanced_path`,
`parse_multiconductor_path`, `parse_ac_scuc_path`, and similar functions. Such a
list grows for every new value type and repeats one operation under many names.
Format named helpers remain private or appear only when a format has options
that the generic parser cannot express. Python and Julia can accept an optional
requested output type because their dispatch and exception models differ from
Rust; they use the same parser underneath.

`diagnostics` is the sole collection of structured findings. Warning is one
diagnostic severity. The duplicate rendered `warnings` field is removed;
messages are rendered from diagnostics when requested.

`retained_source` retains the input needed for exact same format writing and for
consumers that reuse source specific data. It does not alter the typed
electrical model. A parsed module has a retained source. A module constructed
entirely in memory does not, so `source() -> Option<&Source>` states the real
condition. Retained bytes are runtime data and are not serialized into
`.pio.json`.

Current main has two source mechanisms that the earlier discussion conflated.
`BalancedNetwork::source` is optional raw source text retained for a byte exact
same format write. It uses shared ownership, so cloning a network does not copy
the text. The current parsed result also has a typed source specific parse tree,
present only for DOE GO Challenge 3. These are not the same value.

The accepted public rule is that retained source appears only on
`PioModule<T>`, not on either network type. The raw source text currently stored
on `BalancedNetwork` moves into the module's `Source`. The source bytes are
retained once and shared where needed. A byte exact same format write uses an
unchanged module and its source; writing a bare network performs a semantic
write from the typed data.

`Source` is opaque rather than a public file or directory enum. It provides one
or more named immutable buffers. `Source::open` acquires them from a filesystem
path, eagerly or on demand. `Source::from_bytes` accepts one named buffer
already in memory. `parse` accepts and retains the resulting `Source`.

Current main also has two conflicting diagnostic names:

- `StructuredDiagnostic` is the ordinary coded finding emitted by parse,
  transformation, validation, and write operations;
- `network::Diagnostic` describes a proposed old value to new value repair.

The accepted 1.0 name is `Diagnostic` for the ordinary coded finding.
`diagnostics: Vec<Diagnostic>` then means a plain list of the one public
diagnostic record. `Diagnostics` is the private mutable collector used while an
operation emits records; it becomes the public vector when the operation
returns. A repair operation can return typed before and after data, but a module
does not carry a second common repair collection.

Current parsing does not call `BalancedNetwork::repair`. The accepted 1.0 rule
is that parsing preserves an explicitly stated invalid value and returns a
diagnostic. Instance construction rejects values that make the requested
calculation undefined. An explicit repair operation or parse/conversion option
can apply documented safe replacements. Format defined defaults used when a
source omits a value are decoding rules, not repairs to a stated value. Every
applied repair records a structured history entry and a coded `Diagnostic` with
severity, code, message, target, and details.

Each parser, transformation, and explicit edit verifies the module it produces.
Verification reports invalid typed data and internal inconsistencies. It does
not repair them. This follows LLVM and MLIR's separation between constructing
IR, verifying it, and applying explicit transformations.

### Error model

`Diagnostic` and `Error` serve different purposes. A `Diagnostic` is a
serializable user facing finding. It can be a note, warning, or error. `Error`
is the Rust `Result` failure value:
it means the operation did not produce its requested output, implements
`std::error::Error`, and preserves an underlying I/O or library error through
`source()`.

The public record remains `Diagnostic`, not `Warning`, because the same record
reports remarks, notes, warnings, and the error that ended a failed operation.
This matches LLVM and MLIR terminology. The public severities are `Error`,
`Warning`, `Remark`, and `Note`. A remark is a useful standalone message about
a successful operation. A note adds context to another diagnostic. A module
can contain an error severity diagnostic when a representable value fails
validation or cannot serve an intended calculation. A returned Rust `Error`
contains at least one error severity diagnostic, but an error diagnostic does
not by itself mean the operation failed.

Transformations, edits, and repairs create structured descriptive history.
Replayable edits belong to a typed value rather than the common history list.
`Remark` is not a replacement for that record and does not require PowerIO to
emit a duplicate message for every event. Debug logging and transformation
history are not diagnostic severities.

The current Rust enum mixes the implementation cause with the public report and
can expose only one diagnostic code. The 1.0 `Error` is an opaque struct. It
keeps its implementation cause private and owns a nonempty list of
`Diagnostic` records. `Display` renders the first error diagnostic,
`diagnostics()` borrows the full list, and `category()` is derived from that
diagnostic's registered code. The category is not stored a second time.

The stable categories are `Io`, `Request`, `Parse`, `Data`, and `Output`.
`Request` replaces the current overly specific `UnknownFormat` category and
covers an unknown format, an unavailable writer, or a requested output type
that the source does not represent. `Parse` means the selected source could not
be decoded. `Data` means decoded input cannot satisfy the requested
transformation or calculation. The error diagnostic code remains the precise
identity; the category is only the binding and exit status projection.

There is no separate public Rust `ParseError`. That name
would describe the operation, but one parse can stop because source I/O failed,
the format request is unsupported, syntax is malformed, or decoded data is
invalid. The precise diagnostic code and the five categories already
distinguish those cases. A `ParseError` would either repeat that taxonomy or
hide it. Python can still map the `Parse` category to `PowerIOParseError`.

`Error` does not retain a second copy of the source bytes. A module on success
or an error on failure retains the same shared `Source` owner. Diagnostic
locations store a source buffer identifier and byte span rather than Rust
references into another field, avoiding a self-referential module lifetime.

`.pio.json` is a versioned disk schema for `PioModule<PioValue>`, not a direct
Serde dump of the Rust memory layout. Its parser upgrades supported older
schema versions before returning the current types. This preserves the
ability to change Rust memory layout and add language bindings without making the
JSON file a preferred exchange format.

### Parser implementation

The source formats have different lexical rules, so 1.0 does not impose one
tokenizer abstraction on every parser. It imposes the same allocation and
measurement rules on format specific scanners:

- retain one source buffer on `PioModule<T>` and represent text tokens as borrowed
  slices or byte ranges into that buffer;
- parse numeric byte slices directly and avoid allocating a `String` for each
  number or field;
- reserve output tables from row counts or reliable input size estimates;
- scan each source once unless a documented format feature requires another
  pass;
- keep lexical scanning, typed decoding, and source preserving write data
  separate even when the current implementation lives in one file;
- benchmark parse time, peak resident memory, allocation count, and allocated
  bytes on small, large, and malformed inputs.

Deterministic allocation budgets run as pull request gates. Wall time and peak
resident memory are reported performance evaluations because shared CI timing
and operating system memory accounting are noisy.

MATPOWER already follows much of this design: its scanner passes byte slices to
`lexical_core` and does not allocate one string per numerical field. The current
PSS/E, PSLF, PowerWorld AUX, and PyPSA parsers allocate owned strings or complete
row tables while scanning. They need format specific borrowed scanners. OpenDSS
already has a lexer, but its retained token ownership and repeated conversions
still need measurement.

The PyPSA 1.0 work makes the existing profile precise. PowerIO types one
electrical snapshot as `BalancedNetwork`, accepted snapshot-local input series
as `TimeSeries<BalancedNetwork>`, and a fixed network with complete state-only
output as `TimeSeries<OperatingPoint<BalancedNetwork>>`. Intertemporal
calculation data, non-electrical components, investment periods, and
stochastic data are outside that profile. The parser diagnoses them instead of
silently reducing them. Retained source preserves exact same format writing; a
cross-format write reports every unsupported retained section. PyPSA NetCDF
waits for source neutral representations that preserve its full semantics.

Egret's time series encoding is simpler: `system.time_keys` supplies one
ordered list of time labels and a recognized field can replace its scalar value with
`{"data_type":"time_series","values":[...]}`. PowerIO 1.0 supports this
encoding whenever every varying field belongs to the accepted scalar network
profile. The result is `TimeSeries<BalancedNetwork>` because loads, costs,
limits, and availability can vary; it is not an operating point series. It
checks every value count against `time_keys`, and writes the same structure without materializing one
network per time point. If `time_keys` is `["08:00", "09:00"]` and a load's
values are `[10.0, 12.0]`, the load is 10.0 at 08:00 and 12.0 at 09:00.
`time_keys` remains the exact Egret field name; PowerIO does not reuse it as a
general public term.

JSON should continue using `serde_json`'s scanner rather than replacing it with
a PowerIO JSON tokenizer. The current JSON parsers build a complete
`serde_json::Value` tree and some clone subtrees. Typed deserialization or a
custom Serde visitor should decode known fields directly, retaining raw spans
only for unknown data that must round trip. Issue 293 tracks the resulting peak
memory work. Binary parsers use checked cursors over the source byte buffer and
must not search the same input region repeatedly.

### Parse results, problem instances, and solver equations

These are three concrete choices:

- `PioModule.value` says which typed data the source contained;
- a problem instance is the numerical input for one requested calculation;
- a solver chooses the equations or relaxation it will use for that input.

For example, `AcOpfInstance` contains the network, generator bounds and costs,
load, voltage limits, and branch limits required by AC optimal power flow. A
nonlinear polar AC model and SOCWR can both use that same instance.
SOCWR relaxes equations from the nonlinear AC model; it does not require a
second copy of the input data. PowerModels expresses the same distinction as
`solve_opf(network_data, SOCWRPowerModel, optimizer)`.

Formulation markers select a mathematical representation of one problem
instance. `DcOpfInstance` can use `BTheta` or `Ptdf`: the first retains voltage
angle variables, while the second eliminates them through a PTDF matrix. These
are two representations of the same DC OPF input. A bare `Dc` marker would only
restate the instance type and is not useful.

`AcOpfInstance` and `McAcOpfInstance` can use AC polar, AC Cartesian, current
voltage, SOC, or SDP markers where the implementation provides those equations.
A relaxation marker does not create another PowerIO instance. The exact Rust
solve signatures and marker names belong to each downstream solver rather than
the PowerIO 1.0 public API.

Tellegen's current `Problem` enum places `DcPf`, `DcOpf`, `AcPf`, `Acopf`, and
`Socwr` in one list. The first four select the requested calculation and its
input data. `Socwr` selects a relaxation for an AC OPF calculation. Tellegen
should accept the PowerIO instance and select the equations separately. A new
PowerIO instance type is justified only when an operation requires additional
input data that must be exchanged independently of a solver.

### Automatic routing decision

The automatic parser applies the following rule:

1. A format that has one fixed PowerIO result uses that result. MATPOWER returns
   `BalancedNetwork`; OpenDSS returns `MulticonductorNetwork`; DOE GO Challenge 3
   returns `AcScucInstance`; the PyPSA CSV electrical profile returns the
   network, network series, or complete state-only series selected by the
   profile rule above.
   BMOPF returns `McAcOpfInstance` in the 1.0 design; until then the current
   parser returns `MulticonductorNetwork`.
2. A source in a format that can encode either network type returns
   `MulticonductorNetwork` when it carries conductor identities, terminal maps,
   per conductor electrical values, or phase specific equipment.
3. It returns `BalancedNetwork` when the source explicitly declares a
   balanced or positive sequence model and does not carry conductor resolved
   data.
4. If neither result is justified, parsing returns a `Request` diagnostic and
   does not guess. Supporting that source requires a documented profile with a
   deterministic routing rule; Rust does not add `parse_as` to force an
   ontology type.

Examples:

- an OpenDSS line on terminals `.1.2.3.0` has explicit conductor data and parses
  as `MulticonductorNetwork`;
- a MATPOWER branch has one positive sequence `r`, `x`, and charging value and
  parses as `BalancedNetwork`;
- a CGMES source using the IEC 61970 transmission profiles parses as
  `BalancedNetwork`;
- an IEC 61968 distribution source with phase specific equipment parses as
  `MulticonductorNetwork`;
- when PowerFactory DGS support is revived, a DGS source with explicit
  natural coordinate conductor matrices, a neutral conductor, or phase
  specific equipment parses as `MulticonductorNetwork`;
- a DGS source containing only positive and zero sequence values parses as
  `BalancedNetwork`. An explicit multiconductor parse may construct a three
  phase representation from those values and report the assumptions.

The final diagnostic rule is a safety fallback. No currently supported format
needs it. It applies when a future parser cannot justify either network type
from the format definition and source content.

## Transformations and lowerings

`Transformation` is the general term for constructing one typed PowerIO value
from another. A lowering is a transformation from reusable or more general
data to data for a specific calculation. `BalancedNetwork` to `AcPfInstance`
is a lowering because the result selects the power flow boundary
specifications and rejects a network that cannot define that calculation.
Multiconductor to balanced construction is a transformation
between network types, not a calculation lowering. Public function names
identify the concrete result; there is no generic `lower` function.

Every public transformation returns:

```text
output + structured diagnostics
```

The diagnostic collection is present even when empty. Failed operations retain
their diagnostics in the returned error.

Transformations include:

- network to problem instance;
- network or instance to matrix data;
- network or instance to graph data;
- multiconductor network to balanced network;
- extraction of a network from a richer problem instance;
- typed data to a supported external format.

The one call conversion API remains first class:

```text
convert(source, format, destination)
  = parse source
  + transform typed data when needed
  + write target
  + combine diagnostics
```

Same format conversion retains exact source data where the format parser
supports it. Cross format conversion preserves everything the target can
represent and reports each default, approximation, and omission.

Not every pair of public types needs a direct transformation. Composed
transformations should preserve and combine their diagnostics.

### Writing

Writing uses one operation for single file, directory, and memory output,
returns every produced artifact, and preserves diagnostics. An owned
destination avoids a public writer lifetime. `ArtifactPath` is a validated,
nonempty relative path with `/` separators and no root, `.`, `..`, or platform
dependent spelling. The accepted surface is:

```rust
pub struct Destination {
    // Private nonexhaustive representation.
}

Destination::path(path)
Destination::memory(root: ArtifactPath)

pub struct MemoryArtifact {
    name: ArtifactPath,
    bytes: Vec<u8>,
}

pub enum WrittenOutput {
    Path { root: PathBuf, artifacts: Vec<PathBuf> },
    Memory { artifacts: Vec<MemoryArtifact> },
}

pub struct WriteResult {
    output: WrittenOutput,
    diagnostics: Vec<Diagnostic>,
}

write(&module, format, Destination::path(path))?;
convert(source, format, Destination::memory("case")?)?;
```

For a single file format, a path destination names the exact file and a memory
destination names the artifact. For a directory format, either destination
names the output root and every returned artifact is below it. Path writes
refuse an existing target by default, stream into a sibling staging path, and
rename the complete output into place; a failed write removes or reports the
staging path and never exposes a partial target as successful output. Memory
output owns `Vec<u8>` buffers after the module and writer are dropped. Returned
inventories are deterministic, complete, and contain no duplicate paths.

Writers accept `&PioModule<T>` so same format output can use retained source.
A bare typed value is wrapped in `PioModule::new(value)` for semantic writing.
A successful call returns its output and diagnostics; failure returns `Error`
with its diagnostics. The prototype covers MATPOWER style one file output,
PyPSA style directory output, named memory artifacts, collision behavior, and
path traversal rejection.

## External information models

CIM, CGMES, MG-RAVENS, DGS, IIDM, UCTE, and later dynamic formats do not add a
third generic network type. Each documented profile maps to a balanced network,
a multiconductor network, or a calculation type declared by the source. A
collection can provide several named source buffers and a new calculation can
add a nonexhaustive `PioValue` variant. Stable string identifiers keep C,
Julia, Python, and serialized APIs additive.

The exact planned format mappings live in `V1_ONTOLOGY.md`; they are not
repeated here. Experimental branches are implementation references, not 1.0
API definitions.

## Matrix semantics

The public DC matrix API follows PowerModels:

```text
b  = imag(inv(r + im*x))
A  = incidence matrix, branches × buses
B  = A' * Diagonal(b) * A
Bf = Diagonal(b) * A

A[e, from] = +1
A[e, to]   = -1

p_bus    = -B * va + p_shift
p_branch = -Bf * va + b .* shift
p_shift  = A' * (b .* shift)
```

Public names are incidence matrix, bus susceptance matrix, branch
susceptance matrix, `B`, and `Bf`.

`DcPfInstance` and `DcOpfInstance` store the selected public branch
susceptance formula:

```text
SeriesSusceptance      => imag(inv(r + im*x))
TapAdjustedReactance   => -1/(x*tap)
ReactanceOnly          => -1/x
```

The type is `BranchSusceptanceFormula`. Phase shift injection remains a
separate quantity. Documentation states the formulas and does not claim a
universal sign.

### DCPF instance and matrix data

`DcPfInstance` does not store a sparse matrix. It contains or shares the
`BalancedNetwork`, one typed bus specification per bus, the
`BranchSusceptanceFormula`, and the reference conditions. A nonreference bus
specifies net active power; a reference bus specifies voltage angle; an
isolated bus participates in neither equation. These values define the
calculation without making one sparse matrix library part of the instance API.

The instance bus specifications give net active power injection at each
nonreference bus and voltage angle at each reference bus. Nonreference voltage
angles and reference bus active power injections are solution values. The
specified injections use generation minus demand and retain stable bus
identities.

For a fixed instance the linear equations are:

```text
p_bus = specified net bus power injection, generation minus demand
p_bus = -B * va + p_shift
B * va = p_shift - p_bus
```

The final equation is solved after applying the specified reference bus angles.
So the numerical calculation is mostly a bus susceptance matrix and a bus power
injection vector, but it also requires the phase shift injection, reference bus
conditions, and stable bus and branch mappings.

`powerio-matrix` derives these quantities through concrete matrix operations:

- incidence matrix and branch susceptance values;
- bus susceptance matrix `B`;
- branch susceptance matrix `Bf`;
- phase shift injection;
- bus power injection from the instance bus specifications;
- the reference constrained linear system.

The matrix results carry their bus and branch mappings. Matrix users can retain
them, while Tellegen can compile the same instance into private solver data.
Changing only the specified injections or reference conditions updates the
right hand side without reconstructing `A`, `B`, or `Bf`. The C API fills
caller owned bulk buffers so the PowerModels sign conversion does not allocate
another vector.

### AC power flow instance

`AcPfInstance` contains or shares the `BalancedNetwork` and one typed partial
specification per bus. Its bus specifications follow MATPOWER and PowerModels:

- a PQ bus specifies net active and reactive power injection;
- a PV bus specifies net active power injection and voltage magnitude;
- a reference bus specifies voltage magnitude and voltage angle.

The remaining bus quantities are solution values. Generator reactive bounds
remain on the network, but enforcing those bounds during power flow is solver
policy. MATPOWER leaves enforcement off by default and, when requested, fixes a
violating generator at its bound, changes the bus to PQ, and solves again.
PowerModels also builds its ordinary power flow with unbounded generator power
variables. PowerIO therefore does not silently change the instance bus
specification while constructing it.

A controlled bus must have one unambiguous voltage magnitude setpoint. MATPOWER
initializes a controlled bus from generator `VG`; when several generators at
one bus disagree, the last row happens to win. PowerModels instead uses the bus
`vm` setpoint and warns when generator `vg` differs. Neither order dependence nor
silently choosing a different source field is a stable format neutral rule.
Each parser records the source format's controlled voltage value explicitly.
`AcPfInstance` construction rejects conflicting active controllers at one bus
and returns diagnostics identifying every conflicting value. The caller can
resolve the conflict through an explicit edit. Parsing still succeeds and
preserves all source values.

### Zero impedance branches

A zero impedance branch is not a finite admittance. In an exact power flow it
imposes equal terminal voltage and carries an explicit branch flow or current
that enters the two bus balance equations with opposite signs. BMOPFTools uses
this model for a closed switch. Replacing zero by a small number is poorly
conditioned; silently omitting the branch disconnects the network.

PowerIO 1.0 preserves balanced zero impedance branches in the network and
instance types. Balanced finite admittance and susceptance matrix operations
return a coded error when they encounter one; the current default that skips
the branch is removed. The multiconductor passive and augmented matrix rules
are defined separately below because their terminal mappings carry the exact
node identifications and constraint rows.
A solver formulation that implements the equality and branch variable can
consume the instance exactly. A solver that does not implement it returns the
same unsupported data diagnostic rather than changing the network.

`merge_zero_impedance_buses` is an optional, explicit network transformation.
It is allowed only when merging the buses preserves their controls and bounds
and the removed branch has no operational limit that would disappear. It
returns the bus and branch mapping plus diagnostics. It never uses a hidden
threshold: changing a small nonzero impedance to zero is a separate explicit
value edit. The current `reduce_zero_impedance(threshold)` operation is not a
valid 1.0 default because it discards bus data and branch identity and can drop
a binding branch limit.

Merging also removes the original branch flow variable. In a meshed network
that flow need not be recoverable uniquely from the merged solution. This is
why the merge is never applied by `AcPfInstance`, `DcPfInstance`, or a matrix
builder. Exact branch flow and limit support remains the long term solver path;
the explicit merge exists for callers that accept the stated loss.

### AC power flow Jacobian

The exact AC power flow Jacobian belongs in `powerio-matrix`; it is derived from
`AcPfInstance` and its operating point, not stored on the instance. It is
different from the FDPF `Bp` and `Bpp` approximations and from the flat start
linear AC matrix.

PowerIO 1.0 provides one `calc_power_flow_jacobian` operation. Its
`VoltageCoordinates` option is `Polar` or `Cartesian`. The result contains
derivatives of
every bus active and reactive power injection with respect to every voltage
coordinate. It is independent of a solver's choice of free variables and
equations.

This matches MATPOWER `dSbus_dV` and the full form of `makeJac`. In polar
coordinates rows are active then reactive power and columns are voltage angle
then magnitude. In Cartesian coordinates rows are active then reactive power
and columns are real then imaginary voltage. The sparse `2n × 2n` result carries
bus mappings for both dimensions.

PowerModels `calc_basic_jacobian_matrix` is not this same matrix: it replaces
voltage variables with unknown active or reactive generator injections at PV
and reference buses. MATPOWER's reduced Newton matrices likewise select
solver specific rows and columns, and its Cartesian solver adds the specified
voltage magnitude squared equation at PV buses. Those are solver variable and
equation choices. Tellegen or another solver derives them from the physical
Jacobian and `AcPfInstance`; they do not define a second public PowerIO Matrix
operation.

Every complex voltage satisfies `Vm^2 = Vr^2 + Vi^2`, including at PQ buses.
That identity is not another equation when `Vr` and `Vi` are the voltage
coordinates. It becomes a constraint only when voltage magnitude is specified,
as at a PV bus in MATPOWER's Cartesian Newton solver.

Sparse structure is allocated once from the admittance matrix pattern and
numerical values update in place for operating points with unchanged topology.
Compatibility tests compare the physical derivatives directly with MATPOWER
and use them to reproduce the PowerModels mixed variable Jacobian without
making that solver arrangement part of the public result.

### Multiconductor nodal admittance

PowerIO 1.0 includes direct multiconductor matrix construction in
`powerio-matrix`. PowerIO.jl exposes
`calc_admittance_matrix(::MulticonductorNetwork)` through the C matrix path.

The accepted compatibility semantics are:

- one matrix index per `(bus_id, terminal_name)` node, with a deterministic
  node to index map returned with the matrix;
- SI siemens;
- full conductor coordinates with floating neutrals retained and no automatic
  Kron reduction;
- earth as the voltage reference rather than a matrix row;
- `I = Y * V`, using the BMOPF and OpenDSS current direction;
- reciprocal equipment produces `Y == transpose(Y)`, not a Hermitian matrix;
- no hidden replacement of an ideal connection by a large artificial
  admittance.

The public result names and fields are:

```rust
pub enum VoltageReference {
    Earth,
}

pub struct MulticonductorNodalAdmittance {
    y: ComplexSparseMatrix,
    nodes: Vec<MulticonductorNode>,
    terminal_to_node: Vec<(TerminalId, usize)>,
    voltage_reference: VoltageReference,
}

pub struct MulticonductorAugmentedAdmittance {
    system_matrix: ComplexSparseMatrix,
    nodes: Vec<MulticonductorNode>,
    constraints: Vec<VoltageConstraint>,
    terminal_to_node: Vec<(TerminalId, usize)>,
    voltage_reference: VoltageReference,
}
```

`MulticonductorNode` identifies the merged `(bus_id, terminal_name)` set and
`TerminalId` is the original stable terminal identity. The augmented unknown
ordering is node voltages followed by constraint currents; its block matrix is
`[Y C'; C 0]` and its row ordering is node currents followed by zero constraint
right hand sides. The C, Julia, and Python projections use these field names and
ordering.

Closed switches, zero impedance connections, and unity ratio zero leakage
transformers impose equality of their terminal voltages. The passive matrix
builder merges each connected set into one matrix node and returns a map in
which every original terminal identity resolves to that node. It does not add a
large artificial admittance.

Only an exact zero or an element defined as ideal receives this treatment. A
small nonzero impedance remains a finite impedance. Treating it as zero would
be a separate explicit transformation with a diagnostic, not a hidden matrix
tolerance.

A zero leakage transformer with a nonunity ratio imposes
`V_from - ratio*V_to = 0`. A finite passive nodal admittance matrix cannot
express that equation. The passive builder therefore returns a diagnostic that
directs the caller to the augmented builder. The augmented result appends the
constraint and its associated current unknown exactly. These fixed semantics
match BMOPFTools and do not require a policy enum.

BMOPFTools currently exposes three related results:

1. a passive nodal admittance matrix for lines, shunts, capacitors, and
   transformers;
2. a load linearized matrix plus a voltage dependent compensation current;
3. an augmented matrix with constraint rows for ideal switches and
   transformers that have no finite nodal admittance representation.

The 1.0 public matrix API includes the passive nodal admittance matrix and the
exact augmented system. `calc_admittance_matrix(::MulticonductorNetwork)` means
the passive result. The augmented result has a separate explicit entry point
and result type because its constraint rows are not ordinary network nodes. A
single method must not silently approximate ideal equipment.

### Graph data

Graph data preserves equipment identity and parallel connections. The current
balanced `petgraph::UnGraph` path already adds one edge for every in service
branch, including several branches between the same pair of buses. An adjacency
matrix must retain edge counts or weights when a binary matrix would collapse
those branches.

Multiconductor graph data must also preserve conductor terminals and equipment
with more than two terminals. A two endpoint edge representation is not enough
for every transformer or distribution device. This does not justify another
network type: `MulticonductorNetwork` remains the electrical data, and each
graph transformation states its node and edge mappings. More graph result types
can be added after 1.0 without changing either network.

The load linearized result follows after 1.0. Its design accepts a typed steady
state operating point in the same node ordering as the passive result.
A caller passes one operating point to `powerio-matrix`. A typed value that
represents several time points can construct each operating point without
copying the base network; the matrix builder does not parse `PioModule` or
source specific time fields. For unchanged topology, the implementation reuses
the node map and sparse structure and updates numerical values in place where
the sparsity pattern is unchanged.

This replaces duplicate multiconductor matrix assembly in BMOPFTools.
BMOPFTools remains an independent compatibility and validation reference while
the PowerIO implementation is established, then consumes the PowerIO result.

## Evaluation

The current validation workflow already checks parsed fields, conversion
results, balanced admittance matrices, OpenDSS voltage solutions, and BMOPF
schema output against PowerModels, pandapower, PyPSA, Egret, OpenDSS, and
ExaPowerIO. It does not yet establish end to end power flow and optimization
agreement for every source format and PowerIO instance.

The 1.0 evaluation harness extends the existing conversion matrix with four
separate checks:

1. source parse fidelity against an independent parser for the original source;
2. target write and parse fidelity against the target software;
3. numerical data agreement for admittance, incidence, susceptance, limits,
   costs, and operating points;
4. calculation agreement after constructing a PowerIO instance and solving it
   with Tellegen, ExaModelsPower, PowerModels, or another independent solver.

For a format parsed directly by an independent tool, the reference tool must parse
the original source. Converting a file with PowerIO and then giving that output
to both solvers tests instance and solver agreement, but it cannot detect a
shared mistake in the original parse. Both tests are useful and are reported
separately.

Power flow comparisons include convergence status, bus voltage magnitude and
angle, bus injections, branch terminal flows, and equation residuals after
aligning stable element identities. Optimization comparisons include
termination status, objective value, feasibility residuals, bound and network
violations, and dispatch. Dispatch is compared directly only when the optimum
is unique; different optimal points with the same objective and feasibility are
not reported as parser failures.

Each evaluation record states the source, source software version, solver
settings, transformation path, expected diagnostics, identity mapping,
tolerances, and results. Open source oracles run in pull request CI on the
small corpus. Larger corpora and licensed tool outputs run separately, with
redistributable numerical reference results checked into the repository only
when their licenses permit it.

One nonpublished `evals/` workspace contains both correctness evaluations and
performance benchmarks. The current top level `benchmarks/` content moves under
that workspace instead of leaving two overlapping harnesses. A small Rust
executable can provide shared manifest and comparison logic, while Julia and
Python adapters launch their native tools. Runtime crates do not gain solver or
oracle dependencies.

The cross tool corpus moves to `evals/corpus`. Small fixtures needed by
ordinary Rust unit and integration tests stay beside those tests so packaged
crates and isolated crate tests do not depend on the evaluation workspace. The
current `tests/data` directory must be split by use instead of moved wholesale:
shared MATPOWER, PSS/E, and conversion cases move to the corpus; minimal parser
and regression fixtures stay under `tests`.

## `.pio.json`

`.pio.json` is not a case format. It is the versioned JSON serialization of
`PioModule<PioValue>`. Its one typed value can be a network, time series,
scenario set, instance, or solution.

Each `PioModule` has exactly one typed `value`. Several networks, instances,
time series, scenario sets, or solutions never compete for that role.

The exact 1.0 top level object is:

```json
{
  "schema": "powerio.module",
  "version": 1,
  "producer": { "name": "powerio", "version": "1.0.0" },
  "value": { "kind": "balanced_network", "data": {} }
}
```

The optional fields are `sources`, `source_map`, `diagnostics`, `history`, and
`extensions`; the writer omits them when empty. `sources` records module local
IDs, display names, stable format names, byte lengths, and optional SHA-256
digests. Digests use an algorithm and lowercase hexadecimal value, not an
unvalidated string. Retained source bytes and local paths are not stored.
`source_map` uses RFC 6901 targets into `value.data`, a relation enum, and zero
or more half open byte spans so it can represent direct copies, defaults,
aggregation, split fields, generated values, unit conversion, and
transformations. Exact, inferred, unit converted, aggregated, split, and
retained-extra entries require at least one span. Defaulted, synthetic, and
transformed entries can have no span; a transformed entry keeps spans when the
source relation remains meaningful. Every span end is at most its source's
declared byte length. `diagnostics` is the durable finding list. `history` is a
structured description of parse, upgrade, transformation, edit, and repair
operations that produced the current value; it is not replayable. Replayable
revisions require their own typed value. `extensions` is one map whose keys
must be namespaced; extension data cannot affect PowerIO calculations.

There is one document version and no per-value version. Versions are immutable.
A semantic field or value kind that version 1 cannot represent starts in
version 2; future writers continue to emit version 1 for values that remain
version 1 representable. The reader first dispatches on `schema` and `version`,
then decodes the selected exact typed DTO. Current version DTOs reject unknown
semantic fields outside `extensions`. `value.data` is a tagged typed record,
never an untyped `serde_json::Value` used for electrical or calculation data.
Every floating field accepts a JSON number or the existing exact strings
`"Infinity"`, `"-Infinity"`, and `"NaN"`; `null` is not a number. Generated
JSON Schema states this union. The compiling DTOs in `prototype/src/schema.rs`
prove header dispatch, strict fields, nonfinite round trips, source map shapes,
and reference validation. A stored `TimePoint` encodes duration exactly as
unsigned `secs` plus a `nanos` remainder below one billion; its source label is
unchanged. Version 1 is not frozen until every promoted value
has its complete DTO, JSON Schema, round trip fixture, and binding test.

`PioModule` has no catchall `repeated_values`, `derived`, or `solutions` field.
A solution is the primary value when the source or operation represents a
solved calculation. A `TimeSeries<T>` or `ScenarioSet<T>` is the primary value
when the source represents that structure. Stable element identities,
calculation data, and temporal data belong to the typed value whose meaning
they define.
Matrix and graph data are returned by `powerio-matrix`, not stored as common
module fields. PyPSA investment periods are not a public module field.

The current public type name `NetworkPackage` becomes `PioModule` before 1.0.
The `powerio-pkg` and `powerio-diag` crates retire at 1.0. `powerio-core` owns
the dependency neutral module, source and diagnostic types, repeated value
containers, common records, and output destinations below both network crates.
The short `powerio` facade owns `PioValue`, universal format dispatch, the
stored schema, and the legacy upgrade reader.

`PioModule<T>` is both the compiler unit in memory and the result of a
successful parse. Parsing `.pio.json` loads the stored module, appends findings
from the current parse to its diagnostics, and sets its runtime retained source
to that `.pio.json` file.

The JSON schema is separate from the Rust memory layout. Runtime
`PioModule<T>` does not derive the wire representation. The document does not
serialize retained source bytes, row caches, matrices, derived summaries,
validation counts, or platform specific handles.

The 1.0 reader upgrades released 0.9.x `NetworkPackage` files and rejects the
pre 0.9 `schema_version` lineage, which 0.9 already required users to
regenerate. The upgrade is executable and one way:

1. identify the legacy shape by `powerio_version` in `>=0.9.0,<0.10.0`;
2. map `model_kind` and `model` to the value type and payload;
3. turn nonempty `operating_points` into the primary
   `TimeSeries<OperatingPoint<N>>` value instead of keeping a parallel static
   value and series;
4. reject a nonempty literal legacy `study` field with a directed migration
   error. Its unapplied cumulative commits and selectable base state cannot be
   turned into history without choosing a revision. The 0.9 migration command
   materializes an explicitly selected commit to a static package first;
5. map `lowering_history`, source descriptors, source maps, diagnostics,
   repairs, and known
   transformations to the new common records and history;
6. recompute summaries, counts, and derived caches and emit one upgrade
   diagnostic that those legacy fields were nonauthoritative;
7. never reconstruct runtime `Source` ownership from a serialized path or
   retained flag.

Missing `schema_version` is accepted only for that positively identified 0.9
shape. Unknown current fields or value identifiers return a `Request` error
rather than being ignored. Each accepted 0.9 release shape has a frozen upgrade
fixture.

A time series or scenario set can store changes against shared typed data.
Selecting an entry must not serialize a network to generic JSON, deserialize a
second network, or clone all network tables. The 1.0 runtime path resolves
element identities once and reuses shared ownership and numerical arrays.
Element inventory, terminal connectivity, or parameter changes invalidate only
the cached structure that depends on them.

The current `materialize_operating_point` implementation performs the generic
JSON round trip and full value allocation described above. It must be replaced
or kept as a slow standalone export path rather than the calculation path used
for a sequence.

## Solver boundary

PowerIO owns typed source data, networks, problem instances, transformations,
matrix data, graph data, and `.pio.json`.

Tellegen, ExaModelsPower, and BMOPFTools consume problem instances. A solver
owns its variable arrangement, equations, constraints, relaxation choices,
factorizations, caches, and live solution state.

Tellegen does not own public `DcNetwork` or `AcNetwork` types. It consumes the
corresponding PowerIO instance and may retain private solver state around it.
Persistent edits and diagnostics can be serialized in `.pio.json` once the
1.0 schema fixes their exact fields. Time series are typed module values rather
than parallel module fields. Live factorizations and the latest solve belong to
the solver.

The objective type and an efficient builder style edit such as
`with_objective_term` belong to `powerio-prob`. Tellegen may expose a
convenience option that calls that operation, but it does not define another
objective representation or mutate the caller's instance silently. The edit
moves or shares the instance data and its network; it does not clone the
network.

`solve_opf` is one solver operation, not Tellegen's complete long term API.
Power flow, optimal power flow, and unit commitment require different input
and result types. `AcScucInstance` belongs on a future unit commitment solve
path. PowerIO 1.0 defines and parses that input; solving AC security constrained
unit commitment is not required for PowerIO 1.0. The exact Tellegen request enum
and function names remain a Tellegen API decision.

### PowerMCP boundary

PowerIO ships its canonical MCP server. PowerMCP installs and launches that
server, coordinates it with simulator servers, and supplies simulator specific
adapters. PowerMCP must not reimplement PowerIO parsing, transformation,
validation, diagnostics, state application, or matrix semantics.

The latest PowerMCP main has one internal `SolverCase` that resolves every
accepted input to a validated `BalancedNetwork`. It also selects stored states
by calling the current whole module materialization API. That is a useful 0.9
integration layer, but it is not the 1.0 boundary:

- it cannot carry `DcPfInstance`, `AcPfInstance`, `DcOpfInstance`,
  `AcOpfInstance`, `McAcPfInstance`, `McAcOpfInstance`, or `AcScucInstance`;
- it duplicates stored state inventory and diagnostic rendering;
- materializing an operating point serializes and allocates another complete
  `PioModule`;
- reducing every solver input to a balanced network discards the information
  that distinguishes problem instances.

For 1.0, PowerMCP routes a declared PowerIO value to a consumer that accepts
that value. PowerIO owns value inspection, validation, typed operating state
selection, and explicit transformations. A PowerMCP adapter owns only the final
conversion from the accepted PowerIO network or instance into one simulator's
API. A backend that only accepts a file can ask PowerIO to write a temporary
artifact after state selection.

The structured PowerIO network JSON transport remains available for existing
network only bridges. `.pio.json` is the transport for diagnostics, history,
operating points, edits, and problem instances. Neither transport authorizes an
implicit multiconductor to balanced transformation.

The PowerIO MCP surface needed by PowerMCP 1.0 includes:

- value kind and supported operation discovery;
- parse, transform, write, matrix, summary, and diagnostic operations using the
  same names and semantics as the language APIs;
- time series and scenario set inspection and typed entry selection;
- one response diagnostic representation based on `Diagnostic`;
- stable source format, value kind, schema version, and element identity
  fields;
- explicit local path handling for operations performed by PowerIO.

PowerMCP continues to own live simulator projects and vendor operations. A
PowerFactory project, PSS/E saved case, PSCAD project, or running PowerWorld
session does not become a PowerIO source format merely because PowerMCP can
operate it. PowerIO owns a format when it can parse or write the documented
data independently of a live simulator.

## Implementation types that are not public ontology

The following current names are removed from the public API or made private:

- `NormalizedNetwork`
- `IndexedNetwork`
- `NormalizedSolverTables`
- `DcSolverData`
- `DcPowerFlowData`

Private indexing, normalization, cached branch calculations, and transport
arrays may remain when they avoid repeated work or allocations. They are not
types that users must understand.

## Crate ownership

The 1.0 layout follows LLVM's dependency direction: shared source and error
infrastructure sits below every parser, the balanced and multiconductor models
remain in separate crates, and a thin universal entry sits on top. The short
name is the facade crate users add; internals stay in separate crates.

- `powerio-core`: the one shared foundation crate. It owns `Source`, source
  spans, format IDs, `Diagnostic`, `Error`, `PioModule<T>`, common module
  records, `TimePoint`, `TimeSeries<T>`, `ScenarioSet<T>`, and output
  destination and artifact types. It owns no electrical network, domain
  element ID, parser, matrix, instance, solution, `PioValue`, or stored JSON
  DTO. One foundation crate keeps source, diagnostic, module, and output
  ownership together.
- `powerio-tx`: `BalancedNetwork` and balanced format parsers and writers.
  This is the current `powerio` implementation crate, renamed. It depends on
  `powerio-core`.
- `powerio-dist`: `MulticonductorNetwork` and distribution format parsers and
  writers. It depends on `powerio-core`, never on
  `powerio-tx`, and stays lean for distribution only consumers.
- `powerio-prob`: public operating points, problem instance and solution types,
  type specific operating point series constructors, and network to instance
  transformations. It depends on the sibling model crates and the lower common
  representation crate, never on the facade. DOE GO Challenge 3 and
  DeepMind problem parsing live here. BMOPF electrical decoding can reuse
  `powerio-dist`, while `McAcOpfInstance` construction lives here.
- `powerio-matrix`: matrix data and graph data derived from both
  `BalancedNetwork` and `MulticonductorNetwork`. It depends on the model and
  problem crates, never on or through the facade.
- `powerio`: the entry facade. It owns `PioValue`, `PioValueKind`, universal
  format dispatch, `.pio.json`, and its upgrade reader. It re-exports
  `powerio-core` and the
  public network, instance, and solution types, plus matrix types when the
  matrix feature is selected, so `cargo add powerio`
  is the complete compiler and
  Tellegen's existing `use powerio::network::Network` imports keep working
  through re-exports.
- language bindings: the same public semantics without binding specific model
  types in the Rust core.

`powerio-pkg` and `powerio-diag` retire rather than preserving boundaries that
no longer match the 1.0 model. Their reusable types move to `powerio-core`.
The old 0.x packages remain available from crates.io; a final 0.9.1 release can
mark them as retired and point to the 1.0 migration. The split avoids a Cargo
cycle: parsers return `PioModule<T>` from the foundation crate while the facade
owns the enum that mentions values from both network crates. Common
infrastructure never imports the producers that use it.

The exact dependency graph is:

```text
powerio-core
├── powerio-tx
└── powerio-dist

powerio-prob   -> powerio-core + powerio-tx + powerio-dist
powerio-matrix -> powerio-core + powerio-tx + powerio-dist + powerio-prob
powerio        -> powerio-core + powerio-tx + powerio-dist + powerio-prob
powerio        -> powerio-matrix when the matrix feature is enabled
powerio-cli / powerio-capi / powerio-py -> powerio
```

`powerio-tx` and `powerio-dist` remain independently usable. The facade pulls
both network families and problem types so automatic parsing and `PioValue` do
not change with Cargo features. Matrix and graph construction remains optional
because it adds the sparse matrix dependencies and is never a parse result.

## C ABI and language bindings

ABI v5 shipped with PowerIO 0.9.0. The 1.0 removal and replacement of public
parse, diagnostic, instance, module, and matrix data cannot retain the v5
number. PowerIO 1.0 uses ABI v6 and accumulates the breaking changes into that
single increment.

The C surface uses opaque owned handles for modules, values, networks,
instances, solutions, matrix results, numerical arrays, and errors. Every
handle type has `retain` and `release`; `release(NULL)` does nothing. A child
accessor returns an independently owned handle backed by shared Rust data,
so releasing a parent never invalidates a child.

A calculation result handle owns its native numerical arrays. Its accessors
return immutable spans valid until that result handle is released. Caller
supplied fill functions are used only when the requested view changes signs,
units, indexes, layout, or subset. The ABI never allocates one C object per bus
or branch and never creates a second vector only for a sign change. A caller
that needs mutable data requests a copy.

Every fallible function returns a status and writes a `PioError` handle on failure.
There is no thread local error slot and no fixed character buffer as the only
structured error channel. Every `extern "C"` function catches Rust panics.
Concurrent immutable calls on distinct retained handles are allowed. Releasing
the same raw handle concurrently with a call is caller error.

Memory behind an owned handle is released only through that handle's matching
release function. Caller-fill buffers remain caller-owned. Borrowed spans are
never freed directly. An owned string or byte buffer has an explicit PowerIO
release function unless the caller supplied its buffer.

The representative ownership shape is:

```c
int32_t pio_parse_file(
    const char *path,
    PioModule **out,
    PioError **error);

PioModule *pio_module_retain(const PioModule *);
void pio_module_release(PioModule *);

int32_t pio_module_balanced_network(
    const PioModule *,
    PioBalancedNetwork **out,
    PioError **error);

int32_t pio_matrix_values(
    const PioMatrixResult *,
    const double **data,
    size_t *length);

void pio_balanced_network_release(PioBalancedNetwork *);
void pio_matrix_result_release(PioMatrixResult *);
void pio_error_release(PioError *);
```

Python read only memory views retain their Rust owner as the base object. Julia
uses a read only `AbstractArray` wrapper that stores a finalizable owner and the
exact library that created the handle. It never frees through a later global
library selection. `copy` returns an ordinary mutable Julia array.

Julia, Go, and direct C consumers use the C ABI. The Python extension uses
PyO3 directly and shares the same Rust owners without routing through C.
Maturin packages that extension; it is a build and wheel tool rather than a
runtime data layer. All bindings expose the same semantics even when their
implementation path differs.

Julia's ordinary parse call performs automatic format and value detection:

```julia
module = PowerIO.parse(path)
```

It returns `PioModule{T}` for the detected `T`. Methods for matrices,
transformations, and solvers then dispatch on `T`; users do not pass
`PowerIO.BalancedNetwork` as a positional parse argument. Julia uses separate
methods for a path, `IO`, and byte input. An optional `value_type` keyword can
assert the expected result but does not select a different parser.

Python follows the same rule with `powerio.parse(source, value_type=None)`.
Rust remains the defining implementation: each binding calls the same parse
once, queries the returned value kind, and wraps the existing owner without
copying it.

A stable C ABI and a Go wrapper do not compete technically. A Go wrapper is a
maintained Go module that uses cgo to own PowerIO handles, expose Go errors and
typed values, and enforce the C lifetime rules. ABI v6 is its foundation. The
only tradeoff is 1.0 implementation and maintenance scope. The current
recommendation is to include a small Go C ABI client in release evaluation for
1.0, then publish an official Go module after the Rust and C surfaces settle.

Sign conversion and unit conversion occur while filling the requested output
buffer. They do not allocate a second temporary vector. Any borrowed C span is
invalidated only by releasing its documented owner or performing an explicitly
mutating operation on that owner.

## Settled foundation

- PowerIO is compiler and conversion infrastructure, not a solver.
- `PioModule<T>` is the sole successful parse result and contains one typed
  value. `.pio.json` is one versioned serialization of it, not a preferred
  exchange format.
- Rust exposes `parse(Source)`. `Source` provides named immutable byte buffers;
  `powerio::try_into_typed::<T>` moves a dynamic `PioValue` module into a
  concrete module without reparsing or cloning it.
- `Diagnostic` has `Error`, `Warning`, `Remark`, and `Note` severities. Failed
  operations return the common Rust `Error` containing diagnostics and any
  underlying cause.
- `BalancedNetwork` and `MulticonductorNetwork` remain separate. Instances
  contain or share a network, solutions contain or share an instance, and
  solver arrays and caches remain private to the solver.
- Power flow instances contain typed partial boundary specifications; their
  solutions contain complete operating points. `TimeSeries<T>` and
  `ScenarioSet<T>` are generic typed module values, compose as
  `ScenarioSet<TimeSeries<T>>`, and are not common module fields. Their private
  private data can share networks and numerical columns.
- Network to calculation instance construction is a lowering. Transformation
  is the general typed value to typed value term.
- Matrix and graph data remain in `powerio-matrix`. `DcPfInstance` is matrix
  free, direct multiconductor admittance construction is required for 1.0, and
  public numerical results carry stable element mappings.
- Borrowed numerical data is immutable and shared across Rust, C, Julia, and
  Python. ABI v6 contains the complete 1.0 C surface change.
- The crate layout has one `powerio-core` foundation; `powerio-tx` and
  `powerio-dist` are sibling model and parser crates; the `powerio` entry facade
  owns dynamic dispatch and stored JSON. The 0.x `powerio-diag` and
  `powerio-pkg` packages retire at 1.0.

## Implementation entry audit

The final audit closed the implementation blockers with source review and the
compiling crate under `prototype/`:

1. an unconstrained `PioModule<T>`, flat `PioValue`, facade free-function
   narrowing, and source retaining mismatch errors compile across the accepted
   multi-crate layout; the prototype also proves why standard `TryFrom` is not
   coherent there;
2. immutable cheap to clone network handles, generic collections with private
   shared numerical columns, and retained operating point handles compile
   without public traits for memory representation or a `BalancedNetworkData` type;
3. the required source profiles, dynamic value variants, stable identifiers,
   instance fields, and solution fields are fixed above;
4. `.pio.json` has one exact versioned top level object, strict typed value
   DTOs, nonfinite number spellings, checked references, and an explicit 0.9.x
   upgrade floor; complete DTOs for all promoted values are a release gate;
5. multiconductor passive and augmented result names, fields, reference, and
   ordering are fixed;
6. the owned destination prototype covers named memory output, one file and
   directory path output, complete artifact inventories, collision refusal,
   and traversal rejection.

The prototype fixed the shared ownership and module foundations recorded here.
Implementation then proceeded through source profiles and calculation types.
Public fields could still be added to nonexhaustive semantic records when
format work produced source evidence; that did not change the module model.

The DC linear system name, parser profile details, solver request enums, and
evaluation thresholds remained implementation decisions outside this
architecture record.

Deferred until after 1.0:

- a general multiperiod planning instance that types time varying costs,
  limits, reserves, commitments, and investment periods;
- load linearized multiconductor admittance data from a typed operating point;
- balanced to multiconductor construction;
- an official Go module; 1.0 includes only the small C ABI evaluation client;
- multiconductor instance types beyond `McAcPfInstance` and
  `McAcOpfInstance`.
