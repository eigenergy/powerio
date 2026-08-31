# PowerIO 1.0 terminology

Status: historical vocabulary from the design work that informed PowerIO 0.10.
It records the meanings used by the other files in this directory and is not
current API authority.

`PioModule<T>` is the accepted top level PowerIO compiler type. Every successful
parse returns a module. `.pio.json` is one versioned serialization of a module,
not a preferred exchange format.

PowerIO parses, verifies, transforms, and writes power system data. It produces
typed networks, problem instances, matrices, and graph data for solvers and
other tools. `Compiler infrastructure` describes this architecture accurately:
external formats are sources, PowerIO types are representations in memory,
transformations move between those representations, and writers emit target
formats. PowerIO is not a solver, and file conversion is one compiler pipeline
rather than its entire scope.

## External data

`Format` names the rules for encoding external data. Examples are MATPOWER,
PowerModels JSON, BMOPF JSON, DOE GO Challenge 3 JSON, and PyPSA CSV.

`Source` is the retained input to a parse operation. It is an opaque owner or
provider of named immutable byte buffers, following LLVM's source manager
model. `Source::open(path)` acquires input from a file or directory.
`Source::from_bytes(name, bytes)` accepts input already in memory and requires a
name for diagnostics and format detection. Without a declared format, `parse`
detects the format from the source. `with_format` selects a parser explicitly
for ambiguous or mislabeled input while keeping one `parse` signature. The
selected parser still validates the input. A file can be memory mapped and a
directory format can acquire its buffers as needed. The public type does not
promise `Vec<u8>`, eager loading, or a copy of caller supplied bytes. File and
directory describe acquisition, not public enum variants or electrical data
types.

Every external file format reaches its parser as bytes, including text formats.
`from_bytes` takes owned or shared immutable bytes because the returned module
can retain it; accepting a temporary borrowed slice would require a copy or tie
the module lifetime to that slice. Diagnostic locations use a source buffer
identifier and byte span rather than Rust references into another module field.
The returned module or error retains the shared source owner.

`PioModule<T>` contains one typed `value: T` and the records PowerIO produced
with it. Calculation state, time indexed data, and source specific planning
data are not generic module fields. A module parsed from external data retains
its source. A module constructed in memory has no retained source. Retained
bytes are runtime data and are not serialized into `.pio.json`. The stored
common records are `producer`, `sources`, `source_map`, `diagnostics`,
`history`, and namespaced `extensions`. History describes operations that
produced the current value; it is not a replay program. Replayable revisions
need a typed value. Derived summaries, caches, counts, validation runs, and
runtime source ownership are not common stored fields.

Input location words and electrical data words do not overlap:

| Word | Meaning |
|---|---|
| file | one filesystem file from which source buffers can be acquired |
| directory | one filesystem directory from which a format acquires source buffers |
| source | the retained named buffers accepted by `parse` |
| module | the top level typed PowerIO compiler unit |

A `.pio.json` file stores one module. Parsing it acquires one source buffer and
returns `PioModule<PioValue>`. Parsing a PyPSA CSV directory acquires the named
files required by that format; the directory is not called a document.

`Profile` is the documented portion of an upstream format that PowerIO types.
Every field inside a supported profile becomes typed data or produces a
diagnostic. PowerIO does not claim support for the rest of the format.

## PowerIO data

`Network` is reusable electrical network data. The public network types are
`BalancedNetwork` and `MulticonductorNetwork`. They retain element inventory,
terminal connections, fixed parameters, limits, costs, and the source's
declared default state. An operating point can replace the state while sharing
the same network data.

`Storage` means electrical energy storage equipment and its electrical or
energy state. Documentation uses `shared tables`, `shared columns`, or `memory
layout` for Rust implementation details. The word does not describe how a
network is held in memory.

`Instance` is the complete input for one named calculation. An instance
contains or shares its network. The public instance types are
`DcPfInstance`, `AcPfInstance`, `DcOpfInstance`, `AcOpfInstance`,
`McAcPfInstance`, `McAcOpfInstance`, and `AcScucInstance`.

`Solution` is the result of one calculation and contains or shares the
immutable instance it solves. The initial public solution types are
`DcPfSolution`, `AcPfSolution`, `DcOpfSolution`, `AcOpfSolution`,
`McAcPfSolution`, `McAcOpfSolution`, and `AcScucSolution`.

A solution is the module's primary value when the source explicitly represents
a solved calculation. Stored voltages and setpoints without a named calculation
and solution status form an `OperatingPoint`, not a `Solution`. `PioModule`
does not have a generic list of solutions.

`Formulation` identifies the equations or relaxation used to solve an
instance. Examples include B-theta and PTDF for `DcOpfInstance`, and polar,
Cartesian, SOC, and SDP for `AcOpfInstance`. A formulation does not create a
new PowerIO instance type.

`Electrical state` is the instantaneous electrical variables for a specified
equipment configuration: voltages, currents and power flows, and device
injections or dispatch where present.

`OperatingPoint<N>` contains a complete assignment of the independent
instantaneous electrical variables and algebraic equipment settings relative
to a network `N`. Switch position, in-service status, tap and phase shift, and
capacitor step belong here when they change those equations. Derived branch
flows need not be stored. An operating point makes no convergence or solution
claim. A power flow instance instead holds partial boundary specifications;
its solution holds the resulting operating point. A solver can accept an
operating point as an optional initial state. Schedules, objectives, controller
queues, solver status, and restart history are not operating point fields.

`Matrix data` and `graph data` are typed results constructed from a network or
instance. They retain the element mappings needed to interpret their rows and
columns.

`PioModule<T>` places no marker bound on `T`. A Rust application can use
`PioModule<MyValue>` and receive the same source, diagnostic, and history
behavior without registering `MyValue` with PowerIO. There is no public
`ModuleValue` marker or sealing module.

`PioValue` is the flat, nonexhaustive sum enum used only when a source,
`.pio.json`, or a language binding decides which built in concrete value is
present. `PioModule<BalancedNetwork>` contains a `BalancedNetwork` directly;
`PioModule<PioValue>` contains the enum. `PioValueKind` is the nonexhaustive
enum that reports the dynamic value kind. Its `as_str()` method returns the
stable stored and binding string. There is no public `ValueTypeId`.

Only concrete types produced by a 1.0 parser, promised by `.pio.json`, or
required for binding discovery belong in `PioValue`. Other `PioModule<T>` and
container compositions remain ordinary typed Rust values but cannot cross the
automatic parse, `.pio.json`, C, Python, Julia, or MCP boundary until PowerIO
adds a variant, stored DTO, and binding tests. The automatic parser returns
`PioModule<PioValue>`. `powerio::try_into_typed::<T>(module)` checks the variant
and moves it into `PioModule<T>` without a copy. The facade's sealed
`FromPioValue` trait performs that conversion for built in values. It is a
behavioral registry, not a bound on `PioModule<T>`; downstream crates cannot
add a dynamic kind without the matching schema and bindings. A
`ValueKindMismatch` owns the original dynamic module so a caller can inspect it
or try another conversion.

## Time and scenarios

`TimePoint` identifies one position in an ordered series. Its position in the
series establishes order. It stores the exact nonempty upstream label and an
optional nonnegative `std::time::Duration`. A label can be a timestamp, an
interval number, or another source identifier; PowerIO does not impose calendar
or time zone semantics on it. Duration is explicit because interval length
affects energy and cost calculations.

`TimeSeries<T>` is an ordered sequence of complete values of one type `T`. It stores its
time points once. `T` states what varies: a sequence of electrical states is
`TimeSeries<OperatingPoint<N>>`, while a sequence in which network parameters
change is `TimeSeries<N>`. PyPSA calls its time points `snapshots`. Egret calls
its source field `system.time_keys`. Those are upstream names, not additional
PowerIO types.

The public generic type has private fields and no public trait for memory representation.
`TimeSeries<T>::get` returns `(&TimePoint, &T)`; `time_point` and `value` return
the individual references. A type such as `OperatingPoint<N>` can itself
be a small owning handle into shared numerical columns, so selecting or retaining
a point neither copies the network nor materializes numerical tables. This
keeps the generic container ordinary Rust while allowing type specific
constructors and shared columns. The private memory representation can change
without changing
the public type.

A `Schedule` prescribes time associated inputs such as multipliers, setpoints,
bounds, or availability and can also define interpolation, repetition, units,
and target bindings. A schedule is an input. A time series of operating points
contains realized states. `TimeSeries<T>` by itself implies neither schedule
semantics nor coupling between entries. Schedule is descriptive terminology,
not a generic 1.0 Rust type.

Time varying bounds, costs, reserves, and commitments are not operating point
fields. They belong to a calculation type that defines their meaning. In 1.0,
unsupported series are retained for exact writing and diagnosed as
uninterpreted.

`Scenario` is a named alternative or sample. Scenarios have no implied time
order. When probabilities are present, they describe mutually exclusive,
exhaustive alternatives.

`ScenarioSet<T>` is a set of alternatives identified by `ScenarioId`. It has no
implied time order. Its primary lookup is `get(&str)` by that ID; iteration
order is preserved only for deterministic writing. A set either supplies a probability
for every scenario or for none. Alternatives that each vary through time use
`ScenarioSet<TimeSeries<T>>`. A deterministic series remains `TimeSeries<T>`;
it is not wrapped in `ScenarioSet<T>`. Contingencies are named outages or
events inside a security constrained instance; they are not scenarios.

`ScenarioSet<T>` does not encode shared first stage decisions,
nonanticipativity, recourse, or a risk measure. Those belong to a named
stochastic calculation instance. A calculation spanning ordered intervals
uses a named multi-period instance when ramping, commitment, storage balance,
energy windows, or other relationships couple its intervals.

Not every time varying quantity is an operating point. Voltages, injections,
and equipment states can form an operating point. Bounds, availability,
commitment, reserves, and cost data belong to a calculation type that defines
their use. Parsers must not relabel all temporal input as operating points.

PyPSA snapshots index both inputs and results; investment periods and stochastic
scenarios add independent axes and optimization semantics. They are not generic
module fields and a complete PyPSA model is not an operating point series.
PowerIO 1.0 supports a documented PyPSA CSV electrical profile. One snapshot
maps to `BalancedNetwork`; supported snapshot-local inputs map to
`TimeSeries<BalancedNetwork>`; a fixed network with only complete state output
varying maps to `TimeSeries<OperatingPoint<BalancedNetwork>>`. Complete PyPSA support requires source neutral
multi-carrier, multi-period, capacity expansion, stochastic calculation, and
result types. PowerIO does not introduce a source specific `PyPsaModel`.
GridFM uses scenarios. Egret uses time points. PowerIO keeps those distinctions
and shares network data when element identities do not change.

## Operations

`parse` converts a `Source` into `PioModule<PioValue>`. The public Rust API has
no `parse_path` or `parse_as` operation.

`write` encodes typed PowerIO data in a target format.

`convert` parses one format and writes another. It composes parse, any
required typed transformation, and write operations.

`transformation` constructs one typed PowerIO value from another. Examples are
`MulticonductorNetwork` to `BalancedNetwork`, and the `BalancedNetwork` to
`AcOpfInstance` lowering.

`Diagnostic` is one coded finding with severity, code, message, target, and
details. A repair is one explicit operation recorded by structured descriptive
history. A warning is a diagnostic severity, not a separate public record type.

Transformation diagnostic codes use the `TRANSFORM` namespace. The current
public `LOWER` namespace becomes `TRANSFORM` in 1.0 so the operation and its
diagnostics use the same word.

`Error` is the one public Rust operation failure type. It implements
`std::error::Error`, retains the underlying cause when one exists, and exposes
the diagnostics emitted before failure. `ErrorCategory` has the five stable
values `Io`, `Request`, `Parse`, `Data`, and `Output`. An unknown format or
requested output mismatch is `Request`; malformed input is `Parse`; valid input
that cannot satisfy an operation is `Data`.

The operation ledger is exact:

| Operation | Input | Output |
|---|---|---|
| parse | `Source` | `PioModule<PioValue>` or `Error` with diagnostics |
| write | typed PowerIO data and a target format | file or directory plus diagnostics |
| convert | a source and target format | written target plus combined diagnostics |
| transformation | one typed PowerIO value | another typed PowerIO value plus diagnostics |

Public APIs, diagrams, and issue text use `parse`, not `read`, for decoding a
format. Internal byte I/O can use ordinary Rust names such as `Read`; that does
not name a PowerIO operation.

## PowerIO module

`PioModule<T>` is the top level container for one typed PowerIO value. It owns
or shares everything needed to interpret that value and exposes
`value() -> &T` and `into_value() -> T`. Matrix and solver APIs borrow the
concrete value; they do not consume serialized JSON or match `PioValue` when
the concrete type is already known.

A successful parse appends its diagnostics to the module. A failed parse returns
the common `Error` type containing its diagnostics because no valid module
exists. A transformation takes `PioModule<T>`, constructs `PioModule<U>`,
preserves the applicable module data, and records the completed transformation
without a JSON round trip.

`Diagnostic` is data reported to a user and can occur on success. `Error` is
the Rust control flow value returned when an operation fails. `Diagnostic`
uses MLIR's four severities: `Error`, `Warning`, `Remark`, and `Note`. A
`Remark` is a useful standalone message about a successful operation. A `Note`
adds context to another diagnostic. A module can contain an error severity
diagnostic when parsing produced a representable value that fails validation
or is unusable for a requested calculation. A returned Rust `Error` contains
at least one error severity diagnostic; an error diagnostic does not by itself
mean the operation failed.
There is no separate public Rust `ParseError` because parse can fail in the
`Io`, `Request`, `Parse`, or `Data` categories.

History operations are durable structured records, not diagnostic severities.
A remark can summarize one of those records for a user,
but PowerIO does not emit a second message for every recorded event.

`Transformation` is the general term for constructing one typed value from
another. A `lowering` is a transformation from reusable or more general data to
data for a specific calculation. `BalancedNetwork` to `AcPfInstance` is a
lowering: the result selects the power flow boundary specifications and rejects
a network that cannot define that calculation.
Multiconductor to balanced construction is a transformation between network
types, not a calculation lowering. Public functions name the concrete result;
PowerIO does not need one generic `lower` function.

`.pio.json` serializes `PioModule<PioValue>` through a versioned stored schema.
The disk schema is not the Rust memory layout. Parsing an older supported schema
upgrades it to the current types before returning the module.

## Format names

Use one name for each source everywhere:

| Name | Source shape |
|---|---|
| MATPOWER file | file |
| PowerModels JSON | file |
| Egret JSON | file |
| PyPSA CSV directory | directory |
| PyPSA NetCDF | file |
| GridFM Parquet directory | directory |
| BMOPF JSON | file |
| DOE GO Challenge 3 JSON | file |
| DeepMind OPFData JSON | file |
| `.pio.json` | file |

Use `DOE GO Challenge 3`, not abbreviated variants, in the public API and prose.
Use the exact upstream spelling only when naming an existing source field,
module, command, or citation.

## Additive format support after 1.0

New parsers and writers map into existing network, instance, solution, matrix,
graph, or `PioModule` values. Adding a format must not require a new network
type unless the data represents a genuinely different electrical ontology.

Rust enums that list source formats or automatically detected value kinds are
nonexhaustive. C, Julia, Python, and serialized APIs identify formats by stable
names rather than ordinal enum values. An unknown future format or value kind
returns a diagnostic instead of changing the meaning of an existing value.

The same opaque `Source` covers the known extension work:

- PSS/E RAWX, DGS, IIDM, UCTE, and MG-RAVENS can acquire named buffers from a
  file;
- CIM and CGMES collections can acquire several named buffers from a file or
  directory;
- later dynamic data can add new typed values without changing parsing,
  diagnostics, source retention, or conversion terminology.
