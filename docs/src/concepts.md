# Core Concepts

PowerIO uses LLVM and MLIR vocabulary for transport and structure and power
system vocabulary for electrical meaning. The same types and operations appear
in Rust, C, Python, Julia, PowerIO IR, and MCP tools.

## The module

`PioModule<T>` contains one typed value and the records needed to understand
how it was produced:

```text
PioModule<T>
├── value: T
├── diagnostics
├── producer
├── sources and source mappings
├── history
└── extensions
```

Rust, Python, and Julia expose `value` and `diagnostics` as fields or
properties. C exposes borrowed accessors because its values are opaque.
Diagnostics belong to the module, not to the contained network or solution.

The module can retain source bytes at run time so unchanged same format
emission is byte exact. Those bytes are not part of serialized PowerIO IR.

## Electrical values

```text
T
├── BalancedNetwork
├── MulticonductorNetwork
├── OperatingPoint<BalancedNetwork>
├── OperatingPoint<MulticonductorNetwork>
├── TimeSeries<T>
├── ScenarioSet<T>
├── ScenarioSet<TimeSeries<T>>
├── *PfInstance / *PfSolution
├── *OpfInstance / *OpfSolution
└── *ScucInstance / *ScucSolution
```

| Type | Meaning |
|---|---|
| `BalancedNetwork` | A self contained balanced electrical case: equipment identities, terminals, physical parameters, ratings, limits, costs, and the source/default operating assignment. |
| `MulticonductorNetwork` | The corresponding conductor resolved distribution model. |
| `OperatingPoint<N>` | A possibly partial alternate electrical assignment over fixed equipment identities. It makes no claim of completeness or power flow feasibility. |
| `TimeSeries<T>` | Values of one type ordered in time. |
| `ScenarioSet<T>` | Named alternatives of one type, optionally with probabilities and with no implied time order. |
| `*Instance` | A calculation definition assigning fixed inputs, unknowns, bounds, objectives, horizon, contingencies, and formulation choices. |
| `*Solution` | Computed quantities plus formulation identity, termination, validity claims, residuals, multipliers, and objective or bound. |

The two network types are peers. `BalancedNetwork` is where MATPOWER, PSS/E,
PowerModels JSON, and the other balanced formats meet.
`MulticonductorNetwork` is where OpenDSS, PowerModelsDistribution engineering
JSON, and BMOPF meet. Neither is a subtype of the other. An explicit
transformation can calculate a balanced positive sequence equivalent from a
multiconductor network and reports every assumption and loss.

## Networks and operating points

An operating point can override demand, setpoints, dispatch, voltages,
injections, equipment service status, switch positions, transformer taps,
phase shifts, and corresponding multiconductor controls. Missing quantities
resolve to the network's source/default assignment.

An operating point cannot change equipment identities or terminals, physical
parameters such as impedance, ratings or limits, costs or objectives,
commitment and reserve structure, horizon structure, or the equipment set. A
scenario that changes those values contains a network or calculation instance
instead.

Topology means electrical connectivity:

```text
declared terminals
+ equipment service status
+ switch positions
= calculated energized topology
```

Tap and phase shift changes affect equations, not connectivity. Impedance,
rating, and cost changes are not operating point changes.

## Collections

Collections compose without flattened public type names:

```text
TimeSeries<BalancedNetwork>
TimeSeries<MulticonductorNetwork>
TimeSeries<OperatingPoint<BalancedNetwork>>
TimeSeries<OperatingPoint<MulticonductorNetwork>>

ScenarioSet<BalancedNetwork>
ScenarioSet<MulticonductorNetwork>
ScenarioSet<OperatingPoint<BalancedNetwork>>
ScenarioSet<OperatingPoint<MulticonductorNetwork>>
ScenarioSet<TimeSeries<T>>
```

Each operating point entry can contain a different sparse set of overrides.
It roots the shared base network without duplicating network tables. A time
series of networks or calculation instances can contain complete values when
their physical or calculation data differs.

## Calculation instances and solutions

PowerIO separates reusable network data, calculation inputs, and calculated
results. A MATPOWER case stays a `BalancedNetwork`; a caller explicitly
constructs `DcPfInstance`, `AcPfInstance`, `DcOpfInstance`, or
`AcOpfInstance`. A solver consumes that typed instance and returns the
corresponding typed solution module.

`SocwrOpfSolution` is a PowerModels SOCWR relaxation result and objective lower
bound. It is not labeled `AcOpfSolution` unless voltage recovery and AC
residual checks support that claim.

## Diagnostics

Every operation reports structured `Diagnostic` records: a stable dotted
code, severity (`error`, `warning`, `remark`, or `note`), message, and, where
available, a target, source byte spans, related records, and a suggested
action. Branch on the code, never on the rendered message.

Successful operations keep their diagnostics on the returned module or
result. Failed operations return or throw the language's structured PowerIO
error.

## Sources and grid exchange formats

A `Source` owns one or more named immutable byte buffers acquired from a file,
directory, or memory. Rust and C callers build it themselves, because those
languages need an explicit owner for acquired bytes. In Python and Julia a
path, an open file object, or a bytes-like value plays that role directly,
since the value already states where the bytes come from and the interpreter
owns them; [Rust, Python, Julia, and C](languages.md) explains the split.
`parse` detects a grid exchange format from the source name and content
unless the caller supplies a format.

`emit` produces one supported format and returns an `EmitResult`. Its
artifact inventory is the list of artifacts the emission produced: one entry
per file, each with its name and either its bytes, for a memory destination,
or its path, after a filesystem commit. A single file format produces one
artifact; a directory format such as PyPSA CSV, GridFM, or CGMES produces
one per file. The result also states the layout (one file or a directory),
the fidelity (an exact same format echo of retained source bytes, or
canonical fresh output), and the emission diagnostics, which report any loss.

`resolve_format` maps accepted third party spellings to a canonical token and
reports its conventional filename suffix, output layout, and fresh emission
support. It describes formats; value types are named by `PioValue` and its
language counterparts.

PowerIO IR is separate: `serialize` and `deserialize` preserve PowerIO types
and module records. It has one 1.0 document shape and is absent from grid
exchange format discovery.
