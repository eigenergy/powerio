# PowerIO 1.0 design rationale

Status: historical rationale from the design work that informed PowerIO 0.10.
It explains the alternatives considered at that time and is not current API
authority.

The selection criteria are:

1. one meaning per public term;
2. ordinary Rust usage when a custom abstraction does not enforce an
   invariant;
3. no network sized copies on parse, selection, transformation, or child
   access;
4. safe ownership after a parent Rust, C, Python, or Julia object is released;
5. a Cargo graph without cycles or hidden reverse dependencies;
6. a finite, versioned disk and binding interface;
7. source neutral electrical and calculation types;
8. room to add source neutral representations without changing existing
   meanings.

## `PioModule<T>`, `PioValue`, and `PioValueKind`

### Selected meanings

`PioModule<T>` owns one `T`, common diagnostics and history, and optional
runtime source ownership. Rust code that knows `T` uses that type directly.

`PioValue` is a flat, nonexhaustive enum containing the finite set of built in
types that automatic parsing, `.pio.json`, and language bindings can discover
at run time. `PioModule<PioValue>` is the dynamic form. The name `AnyValue` was
rejected because the enum does not contain any Rust value; it contains a
reviewed list of built in PowerIO values.

`PioValueKind` is a nonexhaustive enum with one case per `PioValue` variant.
Its `as_str()` method returns the stable string used by stored JSON and
bindings. It has no public integer representation.

### Why `PioModule<T>` has no marker bound

A marker trait is not a separate Rust language feature. It is a trait used for
classification rather than methods. An empty marker such as:

```rust
pub trait ModuleValue {}
```

would say which types are intended to appear as `T`. It would not validate a
value, select a parser, provide stored JSON, register a binding, or change the
runtime representation.

A sealed trait is a public trait with an inaccessible supertrait:

```rust
mod private {
    pub trait Sealed {}
}

pub trait ModuleValue: private::Sealed {}
```

Downstream code can use a sealed trait as a bound but cannot implement it. That
pattern is useful when a library must control every implementation of a
behavior. It does not fit an empty `ModuleValue` marker: `PioModule` lives below
the crates that define the built in values, and the marker would neither
convert nor validate anything.

An open marker compiles across the graph, but an application can implement it
for any local type. It therefore cannot enforce the closed registration that
motivated the sealed proposal. It also adds a bound to every generic API and
creates an extension promise that automatic parsing, stored JSON, C, Python,
Julia, and MCP do not honor.

The unconstrained generic is the smaller interface:

```rust
pub struct PioModule<T> {
    value: T,
    // private records
}
```

`PioModule<MyType>` is a typed Rust composition. It gains module records and
source ownership. It does not gain parsing, serialization, format writing, or
binding support. Those capabilities are implemented at their actual boundary.
Rust still derives `Send` and `Sync` only when `T` satisfies them, and threaded
or foreign APIs impose the bounds they need. All built in module values have
compile time `Send + Sync + 'static` assertions.

This loses no memory safety or run time check because the marker performed
neither. It avoids treating an empty public trait as validation.

### Why there is no `ValueTypeId`

`ValueTypeId` tried to give every registered type a stable identity. The name
resembles Rust's process local `std::any::TypeId`, while PowerIO needs a stable
domain discriminator for disk and bindings. The dynamic enum already defines
the registered set. `PioValueKind` reports that set without stringly typed Rust
matching or user-created identifier collisions.

Typed code already knows `T` and does not need a run time type identifier.
Dynamic code calls `PioValue::kind()`. Stored JSON and bindings use
`PioValueKind::as_str()`.

### Why the enum is flat

A nested form such as `PioValue::TimeSeries(TimeSeriesValue::...)` groups
names, but it does not remove the need to enumerate every supported concrete
composition. It adds a second match and a second nonexhaustive enum while disk
and binding identifiers still need one complete type string.

The flat enum gives a direct relation:

```text
PioValue variant <-> PioValueKind <-> stored string <-> binding wrapper
```

Broad grouping, when needed for user interfaces, is reported by a category
method on `PioValueKind`; it is not encoded as another payload enum.

### Promotion into `PioValue`

A generic Rust composition and a stable dynamic value are different promises.
PowerIO adds a `PioValue` variant only when a supported parser, `.pio.json`, or
binding requires it. Promotion requires:

- one concrete enum variant;
- one stable `PioValueKind` string;
- a typed stored DTO and upgrade tests;
- C, Python, Julia, CLI, and MCP behavior where those surfaces expose the
  value;
- conversion and compatibility tests.

This is why PowerIO does not publish every Cartesian product of
`TimeSeries<T>` and `ScenarioSet<T>`. Unpromoted combinations remain usable in
typed Rust without becoming permanent disk and binding interfaces.

## Cross crate narrowing

The facade cannot implement standard
`TryFrom<PioModule<PioValue>> for PioModule<BalancedNetwork>`: the standard
trait and both outer types belong to other crates. The public API is therefore
one facade function:

```rust
let parsed = powerio::parse(source)?;
let network: PioModule<BalancedNetwork> =
    powerio::try_into_typed(parsed)?;
```

The function checks `PioValueKind`, moves the concrete value, and moves the
module records without allocation. `ValueKindMismatch` owns the original
dynamic module, reports the expected and actual kinds, and returns the module
through `into_module()`.

The facade's sealed `FromPioValue` trait connects each built in type to one
enum variant and stable kind. This is a real conversion behavior, unlike the
rejected empty `ModuleValue` marker. It is sealed because adding a dynamic kind
also requires a stored DTO and every binding. Users call the free function and
do not import the trait. The reverse conversion is ordinary value mapping:

```rust
let dynamic = typed.map_value(PioValue::from);
```

## Network ownership

`BalancedNetwork` and `MulticonductorNetwork` are public immutable owning
handles. Their fields and shared tables remain private. Cloning either type
is constant with respect to network size and does not duplicate electrical
tables. Instances store a network handle by value, and solutions store an
instance handle by value.

A public `BalancedNetworkData` was rejected because it gives memory layout a
second public network name. It would force users to choose between the network
and its data, expose `Arc` placement as API, and constrain later movement
between one shared owner, shared tables, memory mapped tables, or Arrow backed
tables. None of those choices changes the electrical meaning.

The public guarantee is cheap immutable ownership and snapshot isolation, not
an `Arc` field or pointer identity. Foreign child handles retain the shared data
they need, so freeing a parent module does not invalidate a network, instance,
solution, or numerical array.

## `TimeSeries<T>` and `ScenarioSet<T>` memory representation

Public generic traits can represent a logical `T` through a
borrowed view even when no `T` is stored per entry. They also put memory
bounds, associated view types, and lifetime errors into the public API. A
sealed version has the same sibling crate problem as sealed `ModuleValue`; an
open version freezes an extension interface for memory representation before PowerIO has an
external implementor.

The selected 1.0 API uses ordinary generic containers with real values:

```rust
TimeSeries<T>::get(&self, index) -> Option<&T>
ScenarioSet<T>::get(&self, id) -> Option<&Scenario<T>>
```

Large electrical data is not repeated inside each `T`. An
`OperatingPoint<N>` is a small owning handle containing an index and shared
column owner. Construction allocates numerical columns and one contiguous
handle array, not one allocation per time point. `get` and iteration allocate
nothing. Cloning one operating point increments shared ownership and lets it
outlive the series.

The handle array costs a small fixed number of machine words per entry. The
implementation gates measure this cost against the public GAT alternative
before release. Public traits for memory representation stay absent unless measured data
shows that the handle cost is unacceptable.

Generic containers live in `powerio-core`. `OperatingPoint<N>` and its balanced
and multiconductor operating point builders live in `powerio-prob`, which depends on both
network crates. Sibling crates cannot add inherent methods to a foreign
`TimeSeries` type, so specialized constructors are free functions or builders
owned by `powerio-prob`. The multi-crate prototype checks this placement.

The prototype's former `CollectionError` had no coherent meaning: it covered
network shape, time series dimensions, scenario identity, and probabilities.
Production uses the common `Error` and precise data diagnostics such as shape
mismatch, dimension overflow, duplicate scenario ID, and invalid probability.
`Collection` is not a public PowerIO concept.

## Time, scenarios, and operating state

### Electrical state and operating point

An electrical state contains the instantaneous electrical variables for a
specified equipment configuration: bus or terminal voltages, currents and
power flows, and device injections or dispatch where present.

`OperatingPoint<N>` contains a complete assignment of independent
instantaneous electrical variables relative to network `N` and the algebraic
equipment settings needed to interpret them. Switch position, in-service
status, transformer or regulator tap position, phase shift, and capacitor step
belong here when they change those equations. Derived flows need not be stored.

A power flow instance is not an operating point. Its PQ, PV, reference, or
multiconductor boundary specifications intentionally leave solution variables
unknown. The instance stores those partial specifications. A power flow
solution stores the resulting operating point, termination, and residuals. A
solver initial state is an optional solve input.

An operating point does not contain a time schedule, objective, cost, limit,
controller queue, convergence claim, or solver status. Storage state of charge,
thermal state, commitment duration, controller timers, integrators, and queued
actions are additional equipment or control state. A restartable simulator
checkpoint therefore contains more than an operating point.

### Schedule versus realized state

A schedule prescribes time associated inputs such as multipliers, setpoints,
bounds, or availability. It can also define interpolation, repetition, units,
and target bindings. A time series of operating points records complete
realized electrical states. Applying a schedule, executing controls, and
solving the network can produce those states; the schedule and the states are
not interchangeable.

`TimeSeries<T>` means only an ordered sequence of complete `T` values. It does
not imply interpolation, repetition, input versus result, or coupling between
entries. It can contain networks, operating points, independent calculation
instances, solutions, or other complete values.

### Scenario and contingency

A scenario is a named alternative or sample. `ScenarioSet<T>` represents
independent alternatives or realized samples and has no implied time order.
When probabilities are present, they describe mutually exclusive, exhaustive
alternatives.
`ScenarioSet<TimeSeries<T>>` represents alternative trajectories.

A scenario set does not encode shared first stage decisions,
nonanticipativity, recourse, or a risk measure. Those belong to a named
stochastic calculation instance. A contingency is an outage or event enforced
relative to a common base plan or operating point. It is an indexed part of a
security constrained calculation, not a scenario.

A calculation spanning ordered intervals requires a named multi-period
instance when ramping, startup, storage balance, minimum up or down time,
energy windows, investment lifetime, or other intertemporal relations couple
the intervals. `TimeSeries<SinglePeriodInstance>` is not equivalent.

The unqualified public term `Study` is rejected. It does not identify inputs,
equations, outputs, or temporal coupling. Public types use the calculation
name, such as `AcScucInstance` or a future `McQstsInstance`. The literal legacy
0.9 JSON field named `study` can appear only in upgrade documentation.

## Source mappings

### OpenDSS

An OpenDSS `LoadShape` is a schedule: it contains P and Q multipliers or actual
values, fixed or variable intervals, optional hour values, and application
semantics. Daily, Yearly, Duty, and Time solution modes advance time, apply
schedules, solve, execute controls, sample outputs, and update state.

`TimeSeries<OperatingPoint<MulticonductorNetwork>>` is therefore a valid value
for complete sampled electrical states from a QSTS run. It is not the QSTS
input and is not a complete QSTS result. Reproducing the calculation also
requires schedules and bindings, solution timing, control execution, initial
equipment state, convergence and events, and requested monitor or meter
outputs. A future calculation-faithful representation uses named QSTS instance
and solution types. OpenDSS dynamics also needs differential and control state
and is not reduced to operating points.

The 1.0 OpenDSS parser supports the documented static circuit profile and
diagnoses schedule, calculation, and output instructions outside that profile.
Complete sampled multiconductor operating point series remain a built in
dynamic value for `.pio.json` and direct construction.

### BMOPF

The task force BMOPF format defines a static single period multiconductor OPF.
Its faithful value is `McAcOpfInstance`, not a bare network, because the source
defines controllable injections, bounds, constraints, costs, and an objective.

The BMOPFTools extension adds named scaling series and component bindings.
Each selected time index materializes and solves an independent snapshot, so
its evaluated calculation values can form `TimeSeries<McAcOpfInstance>`.
That composition does not preserve the compact scaling and binding encoding;
retained source or a future typed schedule representation does. The extension
stays typed Rust until PowerIO adopts it as a supported stored and binding
profile. BMOPF `control_profile` names a control law, not a time schedule.

### DOE GO Challenge 3

DOE GO Challenge 3 input contains `network`, `time_series_input`, and
`reliability`. It is one coupled scheduling horizon with commitment, startup,
shutdown, ramping, reserves, energy windows, switching, and contingencies. Its
faithful value is `AcScucInstance`. A time series of AC OPF instances would
discard coupling, and a scenario set would misclassify contingencies. The
output is `AcScucSolution` because it is the result of one coupled
calculation.

### PyPSA

A source specific `PyPsaModel` is rejected. Source formats reveal missing
source neutral concepts; they do not receive private top level representations.

Full PyPSA cannot be represented by the current electrical types alone. PyPSA
includes multiple carriers, arbitrary port links and processes, stores,
snapshot indexed inputs and results, three snapshot weight meanings,
investment periods, capacity decisions, stochastic recourse, and risk terms.
Generic time and scenario containers do not encode those relationships.

PowerIO 1.0 therefore states a precise PyPSA CSV electrical profile. A static
snapshot maps to `BalancedNetwork`; supported snapshot-local input series map
to `TimeSeries<BalancedNetwork>`; a fixed network whose complete electrical
state alone varies maps to
`TimeSeries<OperatingPoint<BalancedNetwork>>`. Intertemporal calculation
tables, non-electrical components, investment data, and stochastic data are
diagnosed and retained for exact same format writing; cross-format projection
reports their loss.
PyPSA NetCDF is not claimed as a complete 1.0 format.

Complete source neutral PyPSA support requires a multi-carrier component
representation plus named operation, capacity expansion, stochastic
calculation, and result types. Those types must also serve other formats before
they enter the ontology. This preserves shared reusable network types while leaving a
clear path for PyPSA to use PowerIO as its electrical format layer.

## Crate placement

`powerio-pkg` dissolves and the current balanced implementation crate becomes
`powerio-tx`. The short `powerio` name is the entry facade.

`PioModule<T>` cannot live only in the facade because transmission,
distribution, and calculation parsers must return typed modules without
depending upward on universal dispatch. A separate diagnostics crate beneath a
second common representation crate is acyclic, but almost every crate would
depend on both and ownership of `Source`, module records, output types, and
errors would be split without an independent reason.

The one shared foundation crate is `powerio-core`. It owns `Source`,
`Diagnostic`, `Error`, `PioModule<T>`, common module records, `TimePoint`,
`TimeSeries<T>`, `ScenarioSet<T>`, and output destination types. It owns no
electrical network, domain element ID, parser, matrix, problem instance,
`PioValue`, or stored JSON DTO. `core` is accurate because this crate contains
both compiler data and I/O support; `ir` would falsely claim that all of those
types are an intermediate representation.

```text
powerio-core
├── powerio-tx
└── powerio-dist

powerio-prob   depends on powerio-core, powerio-tx, and powerio-dist
powerio-matrix depends on powerio-core, powerio-tx, powerio-dist, and powerio-prob
powerio        depends on and re-exports the component crates
```

No lower crate depends on the facade, and the two network crates do not depend
on each other. Direct `powerio-tx` and `powerio-dist` users therefore do not
pull the sibling model, problem instances, or matrices.

Parser ownership follows the richest returned type. Traditional network
parsers live in `powerio-tx` or `powerio-dist`. DOE GO Challenge 3 and DeepMind
problem parsing live in `powerio-prob`. BMOPF electrical decoding can reuse
`powerio-dist`, while construction of `McAcOpfInstance` belongs in
`powerio-prob`. The facade performs format detection and dispatch.

## LLVM and MLIR lessons

PowerIO adopts the parts that match its problem:

- declared acyclic library dependencies;
- a support layer for source ownership, diagnostics, and errors separate from
  in memory representations;
- one owning top level module;
- memory layout separate from serialization;
- symbolic identities and source spans rather than exposed row numbers or
  references into a moving buffer;
- explicit typed transformations with validation and diagnostics;
- on demand derived analyses with invalidation when their inputs change;
- owning handles distinct from borrowed numerical views;
- capability traits only when several types implement actual behavior.

PowerIO does not adopt MLIR's generic operation tree, global context, dialect
registry, generic rewrite system, or independently versioned dialect
bytecode. The built in PowerIO values ship together, so `.pio.json` has one
document schema version rather than a separate version for every value type.

The crate split resembles Support, IR, domain producers, and a top level tool
only in dependency direction. PowerIO public names remain power system names.

## Stored JSON terms

A `.pio.json` document has one top level JSON object with:

- `schema`, identifying the document family;
- `version`, identifying the document schema version;
- `producer`;
- one tagged `value` with `kind` and typed `data`;
- optional sources, source mappings, diagnostics, and history.

There is no additional outer object and no independent per-value schema
version. `PioValueKind::as_str()` supplies the `value.kind` spelling.
The runtime memory representation of `PioModule<T>` does not derive or expose
this disk layout.
