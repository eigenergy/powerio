# Core concepts

PowerIO borrows its structure from compiler infrastructure: source text and
the data a program computes with are different representations, and the way
between them is a small set of explicit, checked operations. Electrical
meaning uses power system vocabulary. The same types and operations appear in
Rust, C, Python, Julia, PowerIO IR, and the MCP server.

## The module

`PioModule<T>` holds one typed value and the records that explain it:

```text
PioModule<T>
├── value: T
├── diagnostics
├── producer
├── sources and source map
├── history
└── extensions
```

Rust reaches the value through `module.value()` and the diagnostics through
the `diagnostics` field. Python and Julia expose `value` and `diagnostics` as
properties. C exposes borrowed accessors, because its values are opaque.
Diagnostics belong to the module, not to the network or solution inside it.

While the process runs, a module can retain the bytes it was parsed from, so
writing it back to its own format reproduces them exactly. Those bytes are
not part of serialized PowerIO IR. Editing the value drops them, so the next
same format write serializes the edited value instead.

## Values

`PioValue` is the closed set of value families a module can carry at the
dynamic boundary: automatic parsing, PowerIO IR, and the C, Python, Julia,
and MCP surfaces.

```text
BalancedNetwork
MulticonductorNetwork
OperatingPoint<BalancedNetwork>
OperatingPoint<MulticonductorNetwork>
TimeSeries<T>
ScenarioSet<T>
DcPfInstance, AcPfInstance, DcOpfInstance, AcOpfInstance,
McAcPfInstance, McAcOpfInstance, AcScucInstance
DcPfSolution, AcPfSolution, DcOpfSolution, AcOpfSolution, SocwrOpfSolution,
McAcPfSolution, McAcOpfSolution, AcScucSolution
GeoLayer
```

| Type | Meaning |
|---|---|
| `BalancedNetwork` | A self contained balanced case: equipment identities, terminals, physical parameters, ratings, limits, costs, and the source's operating assignment. |
| `MulticonductorNetwork` | The conductor resolved distribution model. |
| `OperatingPoint<N>` | A possibly partial alternate electrical assignment over fixed equipment. It makes no claim of completeness or power flow feasibility. |
| `TimeSeries<T>` | Values of one type ordered in time. |
| `ScenarioSet<T>` | Named alternatives of one type, optionally with probabilities and with no time order. |
| `*Instance` | The complete input of one named calculation: fixed inputs, unknowns, bounds, objectives, horizon, contingencies, and formulation choices. |
| `*Solution` | The result of one calculation: computed quantities, termination, residuals, multipliers, and the objective or bound. |
| `GeoLayer` | Coordinates and routes for the elements of a case, as a document of its own. |

The two network types are peers. `BalancedNetwork` is where MATPOWER,
PSS/E, XIIDM, CGMES, and the other balanced formats meet.
`MulticonductorNetwork` is where OpenDSS, PowerModelsDistribution JSON, and
BMOPF meet. Neither is a subtype of the other. An explicit transformation
derives a balanced positive sequence equivalent from a multiconductor network
and reports every assumption and loss.

A `BalancedNetwork` parsed from PSS/E and one parsed from MATPOWER are the
same type with the same meanings. Each format has a documented profile, and
data outside it stays in the retained source and is reported rather than
absorbed. There is no universal network format: the balanced model does not
absorb multiconductor data, other energy carriers, or calculation data.

Rust applications can put any type in a module, `PioModule<MyType>`, and get
the same source, diagnostic, and history behavior. Such a type stays outside
the dynamic boundary until PowerIO adds it to `PioValue`, the IR schema, and
the bindings.

## Operating points

An operating point can override demand, setpoints, dispatch, voltages,
injections, equipment service status, switch positions, transformer taps,
phase shifts, and the corresponding multiconductor controls. A quantity it
does not state resolves to the network's own assignment.

An operating point cannot change equipment identities or terminals, physical
parameters, ratings, costs, commitment or reserve structure, the horizon, or
the equipment set. A scenario that changes those holds a network or a
calculation instance instead.

Topology is calculated: declared terminals, equipment service status, and
switch positions together give the energized connectivity. Tap and phase
shift changes affect the equations, not the connectivity.

## Collections

`TimeSeries<T>` and `ScenarioSet<T>` compose without flattened names:
`TimeSeries<OperatingPoint<BalancedNetwork>>`,
`ScenarioSet<TimeSeries<BalancedNetwork>>`, and the other combinations keep
their structural type. An operating point entry refers to the shared base
network rather than copying its tables, so a series of operating points holds
one network and one sparse set of overrides per point. A series of networks
or instances holds complete values when their physical or calculation data
differs.

Indexing a collection returns the contained value or a view rooted in the
owning module. Nothing reparses and no complete network is copied.

## Calculation instances and solutions

Network data, calculation inputs, and results are separate. A MATPOWER case
parses to a `BalancedNetwork`; the caller constructs a `DcPfInstance`,
`AcPfInstance`, `DcOpfInstance`, or `AcOpfInstance` from it. A solver takes
the instance and returns the corresponding solution. PowerIO never solves.

`SocwrOpfSolution` records a PowerModels SOCWR relaxation and its objective
lower bound. It is not an `AcOpfSolution` unless voltage recovery and AC
residual checks support that claim.

## Diagnostics

Every operation reports `Diagnostic` records. A record carries a stable
dotted code, a severity (`error`, `warning`, `remark`, or `note`), a message,
and, where available, a target, source byte spans, related records, and a
suggested action. Branch on the code, never on the rendered message.

A successful operation keeps its diagnostics on the returned module or
result. A failed operation returns or raises the language's PowerIO error,
which carries the same records.

## Sources, formats, and destinations

A `Source` owns one or more named immutable byte buffers acquired from a
file, a directory, or memory. Rust and C build it, since those languages need
an explicit owner for acquired bytes. Python and Julia take a path, an open
file, or a bytes value directly, because the interpreter already owns them;
[Rust, Python, Julia, and C](languages.md) explains the split. `parse`
detects the format from the source name and content unless the caller
declares one.

`emit` writes one format and returns an `EmitResult`: one artifact per file
produced, each with its name and either its bytes, for a memory destination,
or its path after a write; the layout, one file or a directory; the fidelity,
an exact echo of retained source bytes or fresh canonical output; and the
emission diagnostics, which report each loss. A single file format produces
one artifact. A directory format such as PyPSA CSV, GridFM, or CGMES produces
one per file.

`resolve_format` maps an accepted spelling of a format to its canonical token
and reports the conventional file suffix, the output layout, and whether a
fresh writer exists. It describes formats; values are named by `PioValue`
and its counterparts in each language.

## PowerIO IR

`serialize` writes a module as PowerIO IR and `deserialize` reads it back,
with its types, diagnostics, sources, history, and extensions intact. The
document carries an integer generation that changes only when the serialized
representation changes; it is absent from grid exchange format discovery.
[PowerIO IR](pio-json-schema.md) defines the document and its generation
rule.

## Derived data

Sparse matrices, dense solver rows, factorizations, and caches are analysis
data computed from a value. They carry element mappings back into it, are
never stored in a module, and can change representation without changing any
public meaning.

Every transformation names its input and output types and returns
diagnostics. Multiconductor to balanced conversion moves to a less detailed
representation under stated assumptions. Constructing an instance from a
network moves to a more specific one. Format conversion is `parse` followed
by `emit` at the same level. The decisions PowerIO shares with LLVM and
MLIR, and the ones it does not, are recorded in
[LLVM and MLIR lessons](compiler-ir.md).
