# Distribution networks

A multiconductor network is the conductor level distribution model, and
OpenDSS, PowerModelsDistribution engineering JSON, and BMOPF JSON all parse to
it. When you need a calculation, construct an `McAcPfInstance`,
`McAcOpfInstance`, or `LinDist3FlowOpfInstance` from it explicitly (see
[Calculation instances and solutions](instances.md)).

```julia
using PowerIO
feeder = parse("IEEE13Nodeckt.dss")        # PioModule{MulticonductorNetwork}
net = feeder.value
net.lines[1]                               # terminal maps and the line code reference
net.linecodes[1]                           # per length impedance matrices, SI units
feeder.diagnostics                         # what the reader kept, assumed, or refused
```

The model identifies individual conductors throughout. Buses have ordered
terminals and explicit grounding, lines and switches map conductors between
terminal sets, transformers list their windings with connection kinds, and
loads and generators attach per terminal. Impedance and shunt matrices are in
SI units, per unit length, on line codes. An element the reader has no typed
slot for stays verbatim in the `untyped` table and is reported.

The OpenDSS profile is the static circuit, meaning the element definitions
and their electrical data. Load shapes, solve commands, monitors, and other
calculation instructions are outside it; they stay in the retained source,
are reported as uninterpreted, and survive a same format write byte for byte.

```julia
emit(feeder, "dss", "copy.dss")            # the source bytes, sidecars included
result = emit(feeder, "pmd")               # fresh PMD JSON
result.text
result.diagnostics
```

Nothing converts between the multiconductor and balanced models implicitly.
The balanced positive sequence equivalent is an explicit transformation that
reports its assumptions, such as voltage bases per zone, phase aggregation,
and switch merging, and refuses what it cannot represent:

```rust,ignore
use powerio::transform::{to_balanced, to_balanced_report};

let report = to_balanced_report(&feeder)?;   // readiness, assumptions, losses, diagnostics
let balanced = to_balanced(&feeder)?;        // PioModule<BalancedNetwork>
```

Python exposes the same pair as `module.to_balanced_report()` and
`module.to_balanced()`. The transformation has no C entry point in 0.11, so
PowerIO.jl does not bind it.

Multiconductor admittance matrices build directly from the multiconductor
network through `powerio_matrix::calc_multiconductor_admittance_matrix`,
which is Rust only in 0.11.

LinDist3Flow construction is likewise explicit. A module with an explicit
neutral first passes through the facade's `neutral_kron`; the returned network
records the projection provenance and the module records the transform and its
diagnostics. Instance construction then checks the supported radial model
slice, fixes the voltage reference, and creates the portable input:

```rust,ignore
use powerio::{neutral_kron, to_lindist3flow_opf_instance};

let (reduced, report) = neutral_kron(&feeder)?;
let instance = to_lindist3flow_opf_instance(&reduced)?;
```

`powerio_dist::neutral_kron_reduce` remains available when an application
needs the independently owned network projection without module records.
The projection rejects finite current limits on eliminated neutrals. Such limits
need a constraint on the recovered neutral current, which this reduced network
type cannot represent.

`powerio-matrix` compiles that instance to sparse affine rows, bounds, and
second-order cones. It does not select or invoke a solver, so the same bundle
can be handed to a native or WebAssembly-compatible conic backend.
The supported physical slice, rejected equipment and validation evidence are
recorded in the
[multiconductor LinDist3Flow design review](https://github.com/eigenergy/powerio/blob/main/docs/design/lindist3flow-multiconductor.md).

## BMOPF schema versions

PowerIO v0.11.0 supports **draft BMOPF 0.2**, subject to Task Force
review. The schema version is separate from the PowerIO release and from
PowerIO IR generation 2. Producer provenance records the proposal revision and
schema digest. Previously emitted schema identifiers remain readable aliases.

An unqualified `bmopf-json` emission preserves an unchanged source byte for
byte. An explicit schema version always re-encodes the selected version:

```sh
powerio convert feeder.json --to bmopf-json@0.1.0 -o legacy.json
powerio convert feeder.json --to bmopf-json@0.2.0 -o proposal.json
```

The same format strings work with Rust `powerio::emit`, Python `powerio.emit`,
C `pio_emit`, Julia `emit` and the PowerIO MCP emission tool. Rust also exposes
`BmopfSchemaVersion` and `BmopfEmitOptions` for the distribution writer.

Legacy output relocates proposed-only equipment and transformer fields into
`extras`, with diagnostics. A consumer that ignores those extensions cannot be
assumed to calculate the same network. Proposal output uses the declared tables
and preserves winding ratings, taps, neutral impedances and current-limit data.

Unequal bus phase bounds remain individual values through parsing, generation-2
IR and explicit BMOPF output. Uniform values use `v_min`/`v_max`; unequal values
use `v_min_phase`/`v_max_phase` in the typed Rust model. A present scalar takes
precedence over the corresponding vector. PMD voltage arrays instead follow
terminal order and use engineering voltage units.

The C and Julia bus views expose unequal limits as
`phase_to_ground_voltage_min_v` and `phase_to_ground_voltage_max_v`, while
`voltage_min_v` and `voltage_max_v` represent uniform bounds. C spans borrow
from the network's owner and remain valid for the lifetime of that handle.
The unreleased ABI 7 layout and Julia definitions are updated together.

Parsing, structural schema validity, semantic consistency and computational
support are separate checks. Contradictory versions, invalid dimensions,
unknown electrical references and inconsistent bounds produce error diagnostics.
The native multiconductor admittance builder supports ideal grounded-WYE
transformer coupling and rejects leakage, other winding connections, floating
neutrals, core shunts or tap decisions that need a different formulation.

The BMOPF proposal schema is pinned to
[`664b494`](https://github.com/distribution-system-opt/dsopt-schema/commit/664b494f2ee31ee76f8f78e7852cdb1f1c9a8e7d),
with SHA-256 `74d6c6de3637d52e42a26c4cb0584f51df70d69f360b236cf5e23afaf7669462`.
Fresh 0.2.0 output places the immutable retrieval URL in `meta.$schema` and
records the canonical identity, proposal status, digest and revision under
`meta.provenance.powerio_bmopf`. Existing provenance is preserved; a name
collision uses a numbered producer entry. Reading the former canonical or
version-directory identifiers remains supported without fetching remote data.

### Energy prices

Draft BMOPF 0.2 uses `energy_cost_rate` in $/kWh. Generator entries follow phase
order, as do voltage-source entries. Neutral terminals have no price entry. PowerIO retains source prices in `VoltageSource.energy_cost_rate` and
in generation-2 IR. C and Julia expose `energy_cost_rate_per_kwh`; Python's
voltage-source records expose `energy_cost_rate`.

The reader also accepts the deprecated per-phase `cost` spelling. Explicit
0.1.0 output keeps generator `cost` and relocates source prices to
`extras.voltage_source`, with a diagnostic. Only a consumer that reads that
overlay can use the retained source prices. PowerIO stores the coefficients;
the selected solver determines whether and how they enter its objective.

## LinDist3Flow bindings

Python and Julia expose `LinDist3FlowOpfInstance` and `LinDist3FlowOpfSolution`
as typed module values. `to_lindist3flow_opf_instance` constructs an instance
from a supported phase-only network or multiconductor AC OPF instance.
`instance.metadata` supplies the node and conductor axes, roots, and fixed
reference voltages. Line powers follow the reported parent-to-child direction.
Python positions are zero based; Julia positions are one based.

Solutions expose their instance, termination, objective, and named primal
columns: `terminal_voltage_magnitude_squared`, `line_active_power`,
`line_reactive_power`, `generator_active_power`, `generator_reactive_power`,
`source_active_power`, and `source_reactive_power`. Voltage values use squared
volts and power values use watts or vars. Node and line columns follow the
metadata axes; generator and source columns follow network table order, then
channel order. Each returned column is an independent copy.

C ABI 7 adds `pio_value_lindist3flow_opf_instance`,
`pio_value_lindist3flow_opf_solution`, and
`pio_module_to_lindist3flow_opf_instance`. The generic calculation accessors
read the instance, network, objective, constraints, termination, and solution
columns. `pio_lindist3flow_opf_instance_node_at` and
`pio_lindist3flow_opf_instance_conductor_at` return borrowed typed axis views.
Their strings stay valid while the instance handle remains alive.
Sparse conic preparation and solver adapters remain Rust APIs.

PowerIO 0.11.1 writes these values in IR generation 2. LinDist3Flow adds new
structural type names; readers without those types reject them, while existing
network and calculation records keep their representation.


## Solver adapters, including Tellegen

A solver adapter can retain PowerIO's typed module throughout parsing,
projection, calculation, and result storage:

1. Parse a `MulticonductorNetwork` module and apply explicit neutral Kron
   projection when needed. Keep the projection report and reject unsupported
   equipment through the instance constructor's diagnostics.
2. Construct `LinDist3FlowOpfInstance` and call
   `powerio_matrix::build_lindist3flow_standard_form(instance.value())`.
   The result supplies CSC matrices `p` and `a`, vectors `q` and `b`, and
   ordered zero, nonnegative, and second-order cone blocks for
   `min 1/2 x' P x + q' x` subject to `A x + s = b`, `s in K`.
3. Map those blocks to the selected conic solver. Keep `row_origins`,
   `canonical.variables`, and the instance's node and conductor identities
   for diagnostics and result display. Default solver coordinates use a
   1 MVA per-unit base; the explicit options also support SI coordinates.
4. Decode the primal vector with `lindist3flow_values_from_standard_primal`,
   then construct `LinDist3FlowOpfSolution` with the retained instance,
   termination, decoded values, and objective. Store the result in a module
   and call `powerio::serialize` for IR 2 output.

Tellegen can reuse this Rust boundary for native and WebAssembly adapters.
The remaining consumer work is solver dispatch, capability checks, and
presentation of the supported distribution results. Parsing, neutral
projection, electrical coefficients, physical axes, and primal scaling stay
in PowerIO. New LinDist3Flow types require PowerIO 0.11.1; existing saved
IR 2 networks require no format migration.
