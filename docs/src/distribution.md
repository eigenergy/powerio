# Distribution networks

A multiconductor network is the conductor level distribution model, and
OpenDSS, GridLAB-D, PowerModelsDistribution engineering JSON, BMOPF JSON,
and distribution CIM all parse to it. When you need a calculation, construct an `McAcPfInstance`,
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

Rust `VoltageSource::with_reference_terminal` specifies phase voltages relative
to a declared terminal on the source bus. That terminal can float: it is not
implicitly connected to earth or an external neutral. The multiconductor
matrix builder retains the voltage differences and the equal opposite
reference currents. IR 2 uses a distinct tagged source record, so older readers
reject this new physics; ordinary earth-referenced records are unchanged.
`McAcPfInstance::source_boundary` borrows both voltage endpoints and their
phasor differences. The additive C `PioVoltageSourceBoundaryView` serves
network and PF instance callers without changing the existing ABI 7 layouts.
Legacy C source views reject references they cannot describe. The OpenDSS
writer preserves referenced constraints through exact equivalent circuits.
GridLAB-D and the JSON writers accept an already perfectly grounded reference;
floating references still need a qualified mapping for those targets.
Balanced/LinDist3Flow projections and neutral Kron reduction retain their
explicit reference-source restrictions.

The OpenDSS profile is the static circuit, meaning the element definitions
and their electrical data. Load shapes, solve commands, monitors, and other
calculation instructions are outside it; they stay in the retained source,
are reported as uninterpreted, and survive a same format write byte for byte.

OpenDSS imports represent source impedances as coupled branches with ideal
voltage constraints, and each ordinary line carries its effective matrices at
the snapshot frequency. LineCode assignment and later edits follow script
order. Native `switch=yes` lines retain their finite impedance. Unknown length
units are reported; electrical equivalents do not invent a physical distance.
Canonical DSS output uses the typed frequency, snapshot mode and disabled
controls. It preserves unequal phase voltages, zero EMFs and floating source
returns using exact equivalent circuits. Reimport can introduce auxiliary
components, so canonical output is electrically equivalent rather than a text
fixed point. Unchanged source replay remains byte exact.

Direct `Open`/`Close` commands and explicit `SwtControl.State` are not yet
lowered and cause a parse error. Active `Action`/`Normal` control execution is
excluded and reported. Deferred geometry remains source-only and blocks
numerical use or canonical conversion. These restrictions are compatibility
gaps; they do not establish complete static OpenDSS support. OpenDSS load models
1, 5 and 8 carry a typed `BoundedZip` law, including low-voltage current
interpolation, upper-voltage impedance continuation and ZIP dropout. Model 2
remains constant impedance. The Rust `power_scale` method evaluates per-branch
active and reactive multipliers. IR retains this distinct tagged model; readers
without the configuration reject it. Canonical DSS writes the typed limits, including
native defaults, ahead of stale extras or writer defaults.

BMOPF and PMD currently project these bounded loads to their nominal laws with
a diagnostic. Per-phase differences in bounded ZIP coefficients are retained
in the typed model but currently block DSS emission. The legacy C load views
cannot expose the voltage window and reject these loads; additive binding
views remain unfinished. Models 3, 4, 6 and 7 still need complete typed laws.
These are outstanding compatibility tasks, not supported approximations.

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

## Distribution CIM

The `cim-dist` format reads and writes CIM RDF/XML for unbalanced feeders in
the profile the GridAPPS-D and CIMHub tools exchange: one document in the
`http://iec.ch/TC57/CIM100#` namespace, the iec61970cim17v40 and
iec61968cim13v13 release that CGMES 3.0 also uses, with `ConnectivityNode`
connectivity and per phase equipment. The reader also accepts the older
versioned `CIM-schema-cimNN` namespaces and refuses a document set that mixes
CIM16 and CIM100. Input is one document, or a directory whose top level `.xml`
documents read as one case, with `rdf:about` records extending the ones
another document defines. Files in subdirectories are not part of the case,
and ZIP archives are not read yet. A `.xml` document, a nameless source, or a
directory routes here when it carries per phase classes and no CGMES
`md:FullModel` header.

| CIM | Model |
|---|---|
| `ConnectivityNode`, `Terminal` | buses, with the terminals the equipment uses |
| `ACLineSegment` with `ACLineSegmentPhase` | lines, conductors in `sequenceNumber` order; each phase's `sequenceNumber` names its `PhaseImpedanceData` row and column |
| `PerLengthPhaseImpedance`, `PerLengthSequenceImpedance`, segment `r`/`x` totals | line codes in ohm/m and S/m |
| `EnergyConsumer` with `EnergyConsumerPhase` | loads, per phase power |
| `LinearShuntCompensator` with its phases | shunts; sections multiply the per section admittance |
| the `Switch` classes with `SwitchPhase` | switches with their state and rating |
| `PowerTransformer` with `PowerTransformerEnd`, `TransformerMeshImpedance`, `RatioTapChanger` | transformers of any winding count, with taps |
| `EnergySource` | voltage sources, with the series impedance in `rs`/`xs` extras |

`SinglePhaseKind` A, B, C, and N map to terminals 1, 2, 3, and 4, and the
split phase s1 and s2 to 1 and 2. A conductor beyond those four has no CIM
phase, so the writer reports it. The tank form (`TransformerTank` with
`TransformerTankInfo` catalog data), regulator controls, distributed energy
resources, machines, wire and cable geometry, operational limits,
`LoadResponseCharacteristic`, and `TopologicalNode` only documents are not
mapped yet, and the reader reports each of them: a class with no mapping is
counted per class (`READ.CIM_DIST.CLASS_UNMAPPED`), a property no mapping reads
is counted per class and property (`READ.CIM_DIST.FIELD_UNMAPPED`), and a
segment with no impedance is an error severity finding
(`READ.CIM_DIST.IMPEDANCE_UNRESOLVED`). Conversion to another format refuses
such a segment (`READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED`): its placeholder
line code has zero impedance, which would be a short circuit between its
buses. OpenDSS geometry lines are refused for the same reason.

A network read from distribution CIM records `cim-dist` as its source format.
A same format write of a single document returns its bytes. Canonical output is
one CIM100 document with no `md:FullModel` header. Imported mRIDs pass through
and the rest are UUIDv5 values, and the written numbers are the ones the reader
maps back exactly, so write, read, write is byte stable. The output is not
yet validated against the CIM17 profile schemas or an independent
implementation such as CIM-Graph or CIMHub.

## GridLAB-D profiles and projects

The `.glm` extension selects the PNNL 5.3 profile (`gridlabd`, aliases `glm`
and `gridlabd-pnnl`). Select `gridlabd-arras` explicitly for Arras 4.3. The
profiles differ in source interpretation, including conditional preprocessing;
sharing a file extension does not establish equivalent electrical behavior.

```sh
powerio summary feeder.glm --from gridlabd-arras
powerio convert feeder.glm --from gridlabd-arras --to gridlabd-arras -o copy
powerio serialize feeder.glm --from gridlabd-arras -o feeder.pio.json
powerio convert feeder.pio.json --to gridlabd-arras -o canonical
```

Both GLM targets emit directories. An unchanged same-profile module retains
the entrypoint's original relative path and all acquired include files, byte
for byte. Parsing or writing never executes the simulator. Replay reads only
the retained buffers, even if the original files have since disappeared.
Absolute include paths cannot be relocated byte exactly and are refused.
When a caller supplies external definitions through `glm::PreprocessOptions`,
replay reports that those inputs must accompany the files; the module's
`org.powerio.gridlabd` extension records them. The profile also appears in
source descriptors. The legacy network `source_format` field stays unset so
the existing IR 2 record remains compatible.

Unused passive equipment definitions remain in source with a diagnostic. The
reader follows configuration, conductor and spacing references from instantiated
objects; a connected unsupported device still fails. This lets shared equipment
libraries contain unused transformer or triplex definitions.

Fresh networks, edited modules, IR-restored modules and cross-profile writes
generate `case.glm` from the typed network. Rust's
`powerio_dist::glm::emit_canonical` offers single-file output directly from a
network value. Emission validates physical support before writing any output.

The currently qualified electrical profile covers phase-to-earth sources,
node/meter/load objects, independent P/I/Z load terms, parented loads, and
overhead and underground lines with explicit phase matrices or conductor/spacing
configurations at 50 or 60 Hz. Overhead geometry uses the pinned modified-Carson
constants and enabled image-potential capacitance. The resulting linecode records its calculation
and neutral-reduction provenance; canonical output writes current matrices.
Diagnostics report coefficient calculation and the loss of geometry provenance on
canonical emission. Map coordinates never determine conductor spacing or length.
Three-phase subsets keep their A/B/C identities. Bus earth returns do not
constitute explicit line neutrals. A geometry configuration with a neutral
imports GridLAB-D's reduced phase circuit, with zero neutral voltage at both
ends; it does not provide separately solved neutral voltage/current values.
Underground geometry supports concentric-neutral and tape-shield cables. The
selected dialect matters: PNNL includes a separately declared neutral in the
reduction; pinned Arras omits it for concentric-neutral cables. Arras's tape-shield
model uses the first active phase's neutral parameters for the separate neutral's
self impedance. The reader preserves these native differences in the resulting
matrices and reports the selected interpretation. Arras tape-shield geometry
without a separate neutral is singular and is refused. Unshielded phase-cable
geometry is accepted only for Arras, whose native model omits its separate neutral
and charging capacitance; PNNL rejects that geometry. Canonical output carries the
calculated matrices, so it does not reinterpret geometry under the target fork.

Numerical `iteration_limit` settings and voltage/current dump objects remain in
retained source and are reported; they do not become network components.
Ordinary transformers support three-phase `WYE_WYE` (also numeric `1`) and
single-phase `SINGLE_PHASE` connections with explicit voltages and positive series
resistance/reactance. They map to grounded winding records and retain device
coordinates. Canonical output uses the current winding voltage, resistance and
reactance; source-only configuration objects are not required after an IR round
trip. OpenDSS output explicitly disables its additional anti-floating admittance
for these imported circuits. The total rating can be explicit or the sum of
phase ratings; conflicting totals beyond the native 1% limit fail. Individual
phase nameplate declarations remain in source and are reported.

FBS grounded-wye transformers preserve the finite default core admittance in
the existing `no_load_shunt` winding-2 record. Canonical NR GLM represents it
as a constant-impedance load at the secondary bus, with a diagnostic identifying
its contribution to the transformer port current. OpenDSS uses its equivalent
winding-2 exciting branch. A connected directed FBS tree without an explicit
source uses its unique upstream root's phasors; ambiguous roots, cycles and
implicit-source switch/regulator networks fail. Other transformer connections,
non-unit ordinary-transformer taps and explicit source magnetizing parameters
still need qualification.

For both pinned engines, NR `DELTA_DELTA` (also numeric `2`) stamps three
grounded-wye branches, including zero-sequence transfer. The reader preserves
that native circuit in existing winding records and issues
`READ.GLM.TRANSFORMER_EQUIVALENT`. The `gridlabd_declared_connection` extra keeps
the original label across IR; it does not carry electrical parameters. Canonical
GLM writes the effective `WYE_WYE` circuit and diagnoses the label change.
This is native-engine compatibility, not physical delta winding isolation;
physical delta export and FBS delta transformers still require their own mappings.

NR `WYE_WYE` regulators preserve Type A/B per-phase ratios and series resistance
as single-phase winding records. An object tap overrides its configuration tap.
Manual missing taps use the native neutral position; an active controller needs
explicit taps on every active phase and produces a frozen-control warning.
Unit suffixes follow the selected engine: PNNL's `regulation` field has no unit,
while Arras publishes `%`. Bare numeric values retain their native meaning.
The winding's 1 VA normalization base is marked as defaulted and is not a declared
nameplate rating. The `rating_is_normalization_base` extra preserves that fact in
IR; cross-format writers diagnose loss of that metadata. Device coordinates attach to every resulting phase record.
Canonical GLM emits manual regulator equivalents from current winding values;
OpenDSS uses `xscarray=(0)` to avoid its scalar zero-reactance substitution.
FBS regulators, open-delta connections and banked initialization remain unfinished.

Canonical node convergence is set to `1e-9 V`. The tighter former absolute
setting could prevent IEEE13 from converging despite stable electrical values.
This is an engine stopping criterion, not the validation comparison tolerance.

The pinned IEEE13 and IEEE37 NR feeders are qualified against PNNL, Arras and OpenDSS for
all bus-phase voltages, source currents, line/transformer terminal currents,
terminal powers and losses, and original load-object currents/powers. Checks
include independently edited input, IR-restored canonical output and current
balance at every bus. This evidence applies to those feeders and their documented
edits; it does not establish the unfinished profiles above. IEEE37 starts downstream
of its regulator; its NR delta transformer uses the equivalent described above.
Its parent-directory include uses the existing explicit acquisition-root API. See
`evals/gridlabd/README.md` for the reproducible oracle and measurement conventions.

The legacy IEEE4 grounded-wye step-down feeder has fixed-state qualification
with its original default FBS solver and implicit upstream source. Both pinned
forks and OpenDSS agree on all bus and device-port measurements; deliberately
omitting its small default core current fails the comparison. This does not
qualify every IEEE4 transformer configuration.

IEEE34 and IEEE123 NR also have fixed-state qualification across both forks and
OpenDSS, including per-phase manual capacitor admittances and switch/regulator
states. Their edited-state qualification remains pending. The harness documents
per-feeder stopping settings and numerical comparison budgets.

Other geometry/earth models, split-phase and
additional static devices remain unfinished and fail explicitly. Full OpenDSS
static compatibility is not yet established.

Electrical parents are zero-length bus attachments. Qualified node/meter/load
chains flatten onto their root bus while retaining each load's own P/I/Z law
and geographic point. Canonical output attaches loads directly to that bus.
PNNL 5.3 NR and both FBS profiles are qualified for two- and three-level chains,
including phase subsets, reversed declaration order and intermediate demands.
Arras 4.3 NR rejects electrical parent chains deeper than one level: the pinned
engine can omit descendants or crash, so flattening would not preserve its
source behavior. Direct child loads remain supported. This native profile gap
is unresolved; parsing never silently turns omitted descendants into zero loads.

Loads support legacy A/B/C terms and explicit AB/BC/CA/AN/BN/CN power,
current and impedance terms. Terms add independently, including legacy and
explicit terms on the same connection. Explicit pairs map to two-terminal
typed loads; current terms follow the connection voltage angle. Canonical
output preserves each load's voltage base and can express a complete typed
three-channel delta load as explicit pairs. It reports the resulting component
identity split and any terminal orientation normalization. A load's traditional
`D` flag maps legacy A/B/C terms to AB/BC/CA respectively; its stated nominal
voltage remains phase-to-earth, so the pair voltage base is √3 times that value.
Real/reactive setters (`_real`, `_reac`) update the corresponding complex part
in source order, with native units. They work for legacy and explicit channels.
The `D` flag on node/meter/load objects does not create a neutral conductor or
change the source phasor reference. Incomplete typed delta channel layouts,
split-phase services and explicit line neutrals remain under implementation.

Ordinary-phase `switch` objects import their fixed status, banked or individual
phase states and native contact impedance. Finite closed contacts become exact
uncoupled line equivalents with unit normalization length and a diagnostic;
open and ideal closed contacts remain typed switches. A mixed state splits
into closed and open phase groups, each retaining the original object's point.
Banked initialization applies majority state, uses overall status to break a
two-phase tie, and honors an overall open status. Any change from the stated
phase states is reported; no controller or protection sequence is executed.

The solver setting affects impedance defaults. Both pinned profiles default to
FBS, whose unspecified contacts are ideal. NR uses the module's
`default_resistance` (default `1e-4` ohm); unspecified resistance/reactance become
`+default_resistance` and `-default_resistance`, while explicitly zero NR
components become positive `default_resistance`. Supported numeric global
overrides must be finite and positive. Scalar impedance aliases follow native
assignment order and unit interpretation. Canonical GLM emits finite contacts
as overhead-line equivalents and fixed open contacts as open switches.

Ideal closed contacts select FBS and emit explicit zero contact impedance.
The complete link graph, including open contacts, must form a forest with one
source bus per component. Links are oriented away from their sources, and
disjoint switch phases between the same buses are grouped into one native
object. Endpoint reversal and grouped identities are reported. Matching
component points survive grouping; conflicting points fail explicitly.
Cycles, parallel contacts on the same phase, multiple source buses in one
component and bus phases absent from the incoming link require additional
exact equivalents and currently fail. No small impedance replaces an ideal
constraint, and the input network is unchanged.

For noncommuting line impedance and capacitance matrices, the pinned engines
report incorrect source-end currents in NR and FBS despite correct receiving voltages.
Import reports this native discrepancy and retains the physical pi-line model.
Canonical output replaces native line capacitance with exact phase-pair and
earth-return impedance loads at each end. Conductive or unequal end shunts use
the same equivalent in either solver. End-shunt expansion requires symmetric
matrices; a nonreciprocal equivalent needs additional mapping and fails explicitly.

Generated shunts remain attached to their physical ends when a line reverses
for FBS. Line coordinates stay on the series link. Diagnostics name the
generated loads: physical line port currents, powers and losses include these
loads as well as the series link. Reimport preserves the equivalent circuit
as separate typed loads rather than reconstructing the original line identity.
Native `power_losses` also makes each phase's real loss positive before summing;
with mutual coupling, physical total loss must use signed powers at both ports.

Output without ideal closed contacts selects NR. The pinned engines can return NaN results for
some valid circuits even with a successful process exit. One qualified
impedance case reproduces this with either a native switch or its line
equivalent; FBS solves the same circuit and matches the analytic solution.
Engine validation therefore checks finite voltages, currents and powers, and
reports these NR failures separately from successful electrical comparisons.

GridLAB-D object latitude and longitude map to the network's typed
`component_locations` table. Each point identifies its component table and exact
name, with optional latitude, longitude, datum and point origin. Missing axes
are distinct from zero; an absent datum stays unknown. This geography is
independent of legacy bus drawing positions and line routes. The current GLM
mapping covers root buses, parented loads, lines, fixed switch groups and used
line configurations.
Coordinates on a parented node/meter without a mapped device, unused equipment
or source-only instrumentation still require additional attachment support and
are refused explicitly.

Fresh and edited GLM emission uses current typed points. Declared link locations
remain distinct from the endpoint midpoints the engines calculate at startup;
neither establishes a line route or electrical length. GLM cannot declare a
datum or point origin, so writing those fields reports their loss without
transforming the numeric angles. Other exchange writers and the legacy
GeoLayer extraction report omitted component geography. Cross-format coordinate
mapping and dedicated binding accessors remain unfinished.

Networks without component geography keep their existing IR body. A network
with points uses the additive nested `"powerio.LocatedMulticonductorNetwork"`
record; the electrical network inside it retains the legacy layout. Older IR
readers reject this new record instead of silently losing locations. Rust
accessors borrow the typed table; edits copy shared network tables only on the
first write. IR preserves these points in calculation and collection modules
as well as standalone networks.

Neutral reduction and LinDist3Flow preparation validate point attachments on
entry and exit. A replacement line, shunt or generator keeps the original
device's point, including partial axes, datum and origin. A split line
configuration copies its point to each derived configuration. When a component
is removed, its complete point stays in the corresponding action's
`details.source_location`; it is no longer an attachment in the output network.
These actions are also retained in the network's transformation metadata for
IR storage. Point metadata never changes the transformation's electrical data.
When a later transformation replaces a stored audit, the previous record moves
to the flat `powerio_transform_history` array in network metadata. Each entry
has `metadata_key` and `record`; the established metadata keys still expose the
latest audits. Identical records are not archived again. The archive is bounded
at the module-history limit, and malformed or full archives cause an explicit
error before replacing a record. Returned reports describe only the current
call, while IR retains both current and archived records.

The [GridLAB-D oracle harness](https://github.com/eigenergy/powerio/tree/main/evals/gridlabd)
documents pinned engines, supported physics, canonical and project checks,
and remaining limitations.

## BMOPF schema versions

PowerIO v0.11.0 supports **draft BMOPF 0.2**, subject to Task Force
review. The schema version is separate from the PowerIO release and from
PowerIO IR version 2. Producer provenance records the proposal revision and
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

Unequal bus phase bounds remain individual values through parsing, IR version 2
and explicit BMOPF output. Uniform values use `v_min`/`v_max`; unequal values
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
The native multiconductor admittance builder supports ideal two-winding WYE
transformers with grounded or explicitly floating neutral terminals. Each
phase constrains `(Vp - Vpn) = ratio * (Vs - Vsn)`; the transpose supplies
both phase and neutral currents to nodal balance. Fixed taps enter the ratio.
A one-terminal single-phase winding keeps its implicit-ground meaning;
otherwise the final terminal is the neutral, and only an explicit ground or
closed grounding switch grounds it. The builder rejects leakage, other winding
connections, winding-neutral impedance parameters requiring an additional
circuit, core shunts and tap-control decisions. Malformed winding maps and
invalid ratios fail instead of producing an incomplete matrix.

The BMOPF proposal schema is pinned to
[`664b494`](https://github.com/distribution-system-opt/dsopt-schema/commit/664b494f2ee31ee76f8f78e7852cdb1f1c9a8e7d),
with SHA-256 `74d6c6de3637d52e42a26c4cb0584f51df70d69f360b236cf5e23afaf7669462`.
Fresh output places a PowerIO archive commit URL in `meta.$schema`. Proposal output
records the canonical identity, proposal status, digest and revision under
`meta.provenance.powerio_bmopf`. Existing provenance is preserved; a name
collision uses a numbered producer entry. Reading the former canonical or
version-directory identifiers remains supported without fetching remote data.

### Energy prices

Draft BMOPF 0.2 uses `energy_cost_rate` in $/kWh. Generator entries follow phase
order, as do voltage-source entries. Neutral terminals have no price entry. PowerIO retains source prices in `VoltageSource.energy_cost_rate` and
in IR version 2. C and Julia expose `energy_cost_rate_per_kwh`; Python's
voltage-source records expose `energy_cost_rate`.

The reader also accepts the deprecated per-phase `cost` spelling. Explicit
0.1.0 output keeps generator `cost` and relocates source prices to
`extras.voltage_source`, with a diagnostic. Only a consumer that reads that
overlay can use the retained source prices. PowerIO stores the coefficients;
the selected solver determines whether and how they enter its objective.

## LinDist3Flow bindings

Python and Julia expose `LinDist3FlowOpfInstance` and `LinDist3FlowOpfSolution`
as typed module values. Rust and Python also expose the distinct fixed-dispatch
`LinDist3FlowPfInstance` and `LinDist3FlowPfSolution` types.
`to_lindist3flow_opf_instance` constructs an instance
from a supported phase-only network or multiconductor AC OPF instance.
`instance.metadata` supplies the node and conductor axes, roots, and fixed
reference voltages. It also reports whether the retained conductor graph is
meshed. Line powers follow the reported parent-to-child direction.
Python positions are zero based; Julia positions are one based.

Cycles and parallel lines remain in the LinDist3Flow equations. PowerIO emits
`BUILD.LINDIST3FLOW.MESH_APPROXIMATION` for such an instance because it does
not add angle recovery, loop-consistency equations, circulating-flow
regularisation, or automatic radialisation. Mesh feasibility therefore means
feasibility of this declared linear approximation, not AC feasibility or a
unique physical allocation of branch flows.

`LinDist3FlowBuildOptions::unsupported` selects the component-preparation
policy. `Reject` keeps the strict boundary. `Lower` applies audited static
neutral, switch, line-shunt and capacitor transformations. `Approximate` also
linearizes supported load models and maps IBRs to static generators while
freezing named controls. Bounded ZIP can be projected only when nominal
voltage lies strictly inside its smooth law region, above dropout. The action
retains the original `source_voltage_model` and its bounds; strict preparation
keeps the complete piecewise law. The source network is not mutated.
`Permissive` can additionally omit retained untyped
records. `instance.source_network()` remains the caller's input,
`instance.network()` is the prepared calculation network, and
`instance.preparation()` records every action. The serialized instance stores
the source network and reconstructs the preparation report on read.

Solutions expose their instance, termination, objective, and named primal
columns: `terminal_voltage_magnitude_squared`, `line_active_power`,
`line_reactive_power`, `generator_active_power`, `generator_reactive_power`,
`source_active_power`, and `source_reactive_power`. Voltage values use squared
volts and power values use watts or vars. Node and line columns follow the
metadata axes; generator and source columns follow network table order, then
channel order. Each returned column is an independent copy.

`to_lindist3flow_pf_instance` requires finite prescribed generator/IBR P/Q
(or equal lower and upper bounds), clears the objective, and removes conductor
limits from the conic constraint set. After a converged solve,
`evaluate_lindist3flow_pf_limits` reports retained line apparent-power and
both-end current ratings in VA or A, including loading ratios and overload
flags. These checks explicitly state `constraint_enforced = false`.
Transformer/regulator lowering and their thermal checks remain unsupported;
the preparation report records any selected omission rather than presenting
an incomplete report as comprehensive.

C ABI 7 adds `pio_value_lindist3flow_opf_instance`,
`pio_value_lindist3flow_opf_solution`, and
`pio_module_to_lindist3flow_opf_instance`. The generic calculation accessors
read the instance, network, objective, constraints, termination, and solution
columns. `pio_lindist3flow_opf_instance_node_at` and
`pio_lindist3flow_opf_instance_conductor_at` return borrowed typed axis views.
Their strings stay valid while the instance handle remains alive.
Sparse conic preparation and solver adapters remain Rust APIs.
The fixed-dispatch types add no ABI 7 symbols; they remain available through
Rust, Python, and IR version 2 until a future C ABI revision can add a
fixed symbol set deliberately.

PowerIO 0.11.1 writes the OPF pair in IR version 2, and 0.11.4 adds the
fixed-dispatch pair under the same version. Readers without those structural types
reject them, while existing network and calculation records keep their
representation.


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

For fixed dispatch, use `build_lindist3flow_pf_standard_form`, decode with
`lindist3flow_pf_values_from_standard_primal`, and construct a
`LinDist3FlowPfSolution`. Its constructor evaluates retained ratings after a
converged result; `evaluate_lindist3flow_pf_limits` exposes the same report for
preview. The compiled objective is identically zero; line thermal cones are
intentionally omitted.

Tellegen can reuse this Rust boundary for native and WebAssembly adapters.
The remaining consumer work is solver dispatch, capability checks, and
presentation of the supported distribution results. Parsing, neutral
projection, electrical coefficients, physical axes, and primal scaling stay
in PowerIO. New LinDist3Flow types require PowerIO 0.11.1; existing saved
IR 2 networks require no format migration.

The supported schemas ship in the Rust crate at `schemas/bmopf/`, in the
Python package at `powerio/schemas/bmopf/`, and in each C/Julia library archive
at `schemas/bmopf/`. Their manifests record the exact bytes and upstream
review references. Python can read them with `importlib.resources.files("powerio")`.
Upstream review changes do not alter an installed release's schema. A new
schema interpretation requires a reviewed PowerIO release; a PowerIO release
does not establish Task Force acceptance.
