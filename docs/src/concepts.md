# Core concepts

PowerIO borrows its structure from compiler infrastructure. Source text and
the data a program computes with are different representations, and you move
between them through a small set of explicit, checked operations. The
electrical content is described in ordinary power system terms, and the same
types and operations appear in Rust, C, Python, Julia, PowerIO IR, and the
MCP server.

## The module

`PioModule<T>` contains one typed value and the data that explains it:

```text
PioModule<T>
├── value: T
├── diagnostics
├── producer
├── sources and source map
├── history
└── extensions
```

In Rust you reach the value through `module.value()` and the diagnostics
through `module.diagnostics()`. Python and Julia expose both as properties,
`value` and `diagnostics`, and C exposes borrowed accessors because its values
are opaque. Diagnostics belong to the module rather than to the network or
solution inside it.

While the process runs, a module can keep the bytes it was parsed from, which
is how writing it back to its own format reproduces them byte for byte. Those
bytes are not part of serialized PowerIO IR, and editing the value drops
them, so the next same format write serializes the edited value instead of
the old bytes.

## Values

`PioValue` is the closed set of types a module can contain at the dynamic
boundary, meaning automatic parsing, PowerIO IR, the C, Python, and Julia
bindings, and the MCP server.

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

The two network types are peers, and neither is a subtype of the other.
`BalancedNetwork` is where MATPOWER, PSS/E, XIIDM, CGMES, and the other
balanced formats meet; `MulticonductorNetwork` is where OpenDSS,
PowerModelsDistribution JSON, and BMOPF meet. To get a balanced positive
sequence equivalent from a multiconductor network you call an explicit
transformation, which reports each assumption it made and each thing it lost.

A `BalancedNetwork` parsed from PSS/E and one parsed from MATPOWER are the
same type with the same meanings. Each format has a documented profile, and
data outside that profile is reported and stays in the retained source; it is
not folded into the model. There is no universal network format. The balanced
model does not absorb multiconductor data, other energy carriers, or
calculation data.

A Rust application can put its own type in a module, `PioModule<MyType>`, and
get the same source, diagnostic, and history behavior. That type stays
outside the dynamic boundary until PowerIO adds it to `PioValue`, the IR
schema, and the bindings, so until then it cannot pass through PowerIO IR or
reach the other languages.

## Operating points

An operating point can override demand, setpoints, dispatch, voltages,
injections, equipment service status, switch positions, transformer taps,
phase shifts, and the corresponding multiconductor controls. Any quantity it
leaves out resolves to the network's own assignment, which is why an
operating point can be partial.

It cannot change equipment identities or terminals, physical parameters,
ratings, costs, commitment or reserve structure, the horizon, or the
equipment set. If your scenario changes any of those, it contains a network
or a calculation instance instead of an operating point.

Topology is calculated from the declared terminals, equipment service status,
and switch positions, which together give the energized connectivity. Tap and
phase shift changes affect the equations and leave the connectivity alone.

## Collections

`TimeSeries<T>` and `ScenarioSet<T>` nest without flattened names, so
`TimeSeries<OperatingPoint<BalancedNetwork>>`,
`ScenarioSet<TimeSeries<BalancedNetwork>>`, and the other combinations keep
their structural type. An operating point entry refers to the shared base
network instead of copying its tables, so a series of operating points
contains one network and one sparse set of overrides per point. A series of
networks or instances contains complete values when their physical or
calculation data differs.

Indexing a collection returns the contained value or a view rooted in the
owning module; nothing reparses and no complete network is copied.

## Calculation instances and solutions

Network data, calculation inputs, and results are separate types. A MATPOWER
case parses to a `BalancedNetwork`, and you construct a `DcPfInstance`,
`AcPfInstance`, `DcOpfInstance`, or `AcOpfInstance` from it. A solver takes
the instance and returns the corresponding solution; PowerIO itself never
solves.

`SocwrOpfSolution` is a PowerModels SOCWR relaxation together with its
objective lower bound. It is not an `AcOpfSolution` unless voltage recovery
and AC residual checks support that claim.

## Diagnostics

Every operation reports diagnostics. Each `Diagnostic` has a stable dotted
code, a severity (`error`, `warning`, `remark`, or `note`), a message, and,
where available, a target, source byte spans, related diagnostics, and a
suggested action. Branch on the code rather than on the rendered message; the
code is the part that stays stable.

A successful operation keeps its diagnostics on the returned module or
result. A failed operation returns or raises the language's PowerIO error,
which contains the same diagnostics.

## Sources, formats, and destinations

A `Source` owns one or more named immutable byte buffers read from a file, a
directory, or memory. In Rust and C you build it yourself, since those
languages need an explicit owner for the bytes. Python and Julia take a path,
an open file, or a bytes value directly, because the interpreter already owns
them; [Rust, Python, Julia, and C](languages.md) explains the split. `parse`
detects the format from the source name and content unless you declare one.

`emit` writes one format and returns an `EmitResult`. The result contains one
artifact per file produced, each with its name and either its bytes (for a
memory destination) or its path after a write; the layout, which is one file
or a directory; the fidelity, which is either an exact echo of retained
source bytes or fresh canonical output; and the emission diagnostics, which
report each loss. A single file format produces one artifact, and a directory
format such as PyPSA CSV, GridFM, or CGMES produces one per file.

`resolve_format` maps an accepted spelling of a format to its canonical token
and reports the conventional file suffix, the output layout, and whether a
fresh writer exists. It describes formats only; values are named by
`PioValue` and its counterparts in each language.

## PowerIO IR

`serialize` writes a module as PowerIO IR and `deserialize` reads it back
with its types, diagnostics, sources, history, and extensions intact. The
document has an integer generation that changes only when the serialized
representation changes. PowerIO IR is absent from grid exchange format
discovery, so `parse` does not accept it. [PowerIO IR](pio-json-schema.md)
defines the document and its generation rule.

## Derived data

Sparse matrices, dense solver rows, factorizations, and caches are analysis
data computed from a value. They keep element mappings back into that value,
they are not stored in a module, and their representation can change without
changing any public meaning.

Every transformation declares its input and output types and returns
diagnostics. Multiconductor to balanced conversion moves to a less detailed
representation under stated assumptions, while constructing an instance from
a network moves to a more specific one. Format conversion is `parse` followed
by `emit` at the same level.
[LLVM and MLIR lessons](compiler-ir.md) lists the decisions PowerIO shares
with LLVM and MLIR and the ones it does not.
