# Distribution networks

A multiconductor network is the conductor level distribution model, and
OpenDSS, GridLAB-D, PowerModelsDistribution engineering JSON, BMOPF JSON, and
distribution CIM all parse to it. When you need a calculation, construct an
`McAcPfInstance`, `McAcOpfInstance`, `LinDist3FlowPfInstance`, or
`LinDist3FlowOpfInstance` from it explicitly (see
[Calculation instances and solutions](instances.md)).

The explicit [SINCAL multiconductor profile](sincal.md) also produces this
existing type, including when its selected powers happen to be symmetric.
Required unsupported SINCAL components reject the entire parse rather than
returning a partial network. Its balanced profile belongs to the separate
transmission backend; neither path silently falls back to the other.

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

Names from other formats can be free text (a CIM `IdentifiedObject.name`
such as `Cable type A`), but OpenDSS splits a token on whitespace, `,`, `=`,
quotes, and brackets, reads `!` and `//` as comment openers, and reads a `.`
in a bus reference as the node separator. Canonical DSS output replaces each
such character with `_` in the element, line code, bus, and circuit name and
in every typed reference to it, adds a numeric suffix when the result is
already taken in the same OpenDSS class, and reports each replacement
(`EMIT.DSS.VALUE_SUBSTITUTED`). The written deck reads back with every
element connected; the model keeps the original names.

Direct `Open`/`Close` commands and explicit `SwtControl.State` on lines
preserve their fixed terminal states through equivalent circuits. Other device
classes remain unresolved. Active `Action`/`Normal` control execution is
excluded and reported. Unsupported geometry blocks numerical use or canonical
conversion. These restrictions do not establish complete static OpenDSS support.

OpenDSS load models 1, 5 and 8 carry a typed `BoundedZip` law, including
low-voltage current interpolation, upper-voltage impedance continuation and ZIP
dropout. Model 2 remains constant impedance. Models 3 and 4 carry a typed
`BoundedExponential` law: their middle-band P and Q exponents are respectively
`(0, 2)` and the native CVR exponents (default `(1, 2)`). Outside the band, both
powers follow the native fallback independently of those exponents. The Rust
`power_scale` method evaluates these per-branch multipliers. IR retains the
distinct tagged models; readers without a configuration reject it. Canonical DSS
writes the typed exponents and voltage limits ahead of stale extras or writer
defaults. Unequal per-branch parameters emit separate single-phase loads.

PowerModelsDistribution engineering JSON imports every component section the
typed model has a slot for. Its `time_series` section has none: the series
stay in the `untyped` table with a diagnostic, and a fresh PMD write restores
them together with each component's `time_series` references. A multinetwork
document, the output of PMD's `make_multinetwork` with one network per time
step under `nw`, parses to `PioModule{TimeSeries{MulticonductorNetwork}}`,
one network per step in `mn_lookup` time order. Each time point is labeled by
its `nw` key and carries the step's `time_elapsed` as its duration. Reading
such a document as a single network is refused with a message saying so.

BMOPF and PMD project voltage-bounded loads to their nominal laws with a
diagnostic; PMD also diagnoses its lack of exponential load fields. The
multiconductor IVR OPF and LinDist3Flow preparations refuse these loads by
default; each has an explicit opt-in that projects to the nominal law with a
diagnostic (`BoundedLoadLawPolicy::ProjectToNominal`, and the `approximate` or
`permissive` LinDist3Flow policy), described in the multiconductor IVR guide. Legacy C
load views cannot expose the voltage window and reject these loads; additive
binding views remain unfinished. Models 6 and 7 still need complete typed laws.

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
| `TransformerTank` with `TransformerTankEnd` | one transformer per tank, grouped by its `PowerTransformer` in the `cim_power_transformer` extra |
| `Asset` linking a tank to `TransformerTankInfo`, with `TransformerEndInfo` | winding ratings, connections, and resistances |
| `ShortCircuitTest` | short circuit reactances between winding pairs, and winding resistances when no end info states one |
| `NoLoadTest`, `TransformerCoreAdmittance` | the `%noloadloss` and `%imag` extras |
| `EnergySource` | voltage sources, with the series impedance in `rs`/`xs` extras |

`SinglePhaseKind` A, B, C, and N map to terminals 1, 2, 3, and 4, and the
split phase s1 and s2 to 1 and 2. A conductor beyond those four has no CIM
phase, so the writer reports it.

A tank end's terminals follow from its `TransformerTankEnd.phases`, in the
form the dss reader gives the equivalent OpenDSS transformer: a line to
neutral end `A` is terminals `1, 4` (`bus.1.0`), a line to line end `AB` is
a delta winding on `1, 2`, a three phase end is `1, 2, 3` in delta or
`1, 2, 3, 4` in wye, and the center tap halves `s1` and `s2` are `1, 4` and
`4, 2` (`bus.1.0` and `bus.0.2`), so they stack to the full secondary
voltage. Catalog values use `ratedU`, `ratedS`, and `r` in V, VA, and
ohm. `ShortCircuitTest.leakageImpedance` is the standard impedance magnitude
in ohm referred to the energised end; the pair reactance is derived with
`X = sqrt(|Z|² - R²)` after resolving the winding resistances, including when
a test loss is zero/unstated. Test losses are in kW;
`NoLoadTest.excitingCurrent` is percent of rated current at the test's
`basePower`, and percentages are rescaled to winding 1's rating. The
producer-specific GridAPPS-D convention that places X in this magnitude field
is not supported or automatically inferred; those inputs need normalization
to the standard convention before import. End
resistances (`TransformerEndInfo.r`) take precedence over resistances split
from test losses, a loss of 0 states nothing, and a nonzero loss that
disagrees with the end resistances is reported
(`READ.CIM_DIST.VALUE_CONFLICT`).

Tank phase clocks are validated against the represented winding layout:
aligned connections use clock 0, the supported three-phase D/Y lagging
connection uses clock 1, and reversed single-phase center-tap halves use
clock 6. Other vector groups (including clock 11), invalid clocks, inactive
tanks or parents, and disconnected end terminals are refused. An absent clock
is assumed to match the layout with a diagnostic. Fresh writing derives the
clock from the current windings and reports conflicting retained metadata.

A transformer whose extras carry
`cim_form = "tank"` is written as tanks, grouped under the `PowerTransformer`
its `cim_power_transformer` extra names, with tanks of identical catalog data
sharing one `TransformerTankInfo`; every other transformer, including those
from other formats, takes the `PowerTransformerEnd` form.

Regulator controls, distributed energy resources, machines, wire and cable geometry, operational limits,
`LoadResponseCharacteristic`, and `TopologicalNode` only documents are not
mapped yet, and the reader reports each of them: a class with no mapping is
counted per class (`READ.CIM_DIST.CLASS_UNMAPPED`), a property no mapping reads
is counted per class and property (`READ.CIM_DIST.FIELD_UNMAPPED`), and a
segment with no impedance is an error severity finding
(`READ.CIM_DIST.IMPEDANCE_UNRESOLVED`). Conversion to another format refuses
such a segment (`READINESS.CIM_DIST.IMPEDANCE_UNRESOLVED`): its placeholder
line code has zero impedance, which would be a short circuit between its
buses. OpenDSS geometry lines are refused for the same reason;
[Electrical readiness](electrical-readiness.md) describes both checks.

Disconnected or out-of-service switches read as open, preserving connectivity.
For other mapped equipment, the reader refuses an out-of-service state or a
disconnected terminal because the 0.11 model cannot represent that electrical
status. `inService` takes precedence over `normallyInService`. Per-phase delta
compensators and incomplete-phase delta banks are refused rather than converted
to phase-to-ground admittances; balanced three-phase delta banks remain supported.

A network read from distribution CIM records `cim-dist` as its source format.
A same format write of a single document returns its bytes. Canonical output is
one CIM100 document with no `md:FullModel` header. Imported mRIDs pass through
and the rest are UUIDv5 values, and the written numbers are the ones the reader
maps back exactly, so write, read, write is byte stable. The output is not
fully qualified against external producer corpora. The committed synthetic
tank cases are checked against native OpenDSS grounded admittance stamps and
loaded coil phasors by `evals/validation/validate_cim_tanks.py`, including fresh
CIM writing and a clock-1/clock-11 control. This does not establish support for
producer dialects outside the documented electrical profile.

### Distribution CIM status

- **CIM release.** CIM100 (`http://iec.ch/TC57/CIM100#`): IEC 61970
  cim17v40 with IEC 61968 cim13v13. The reader also takes the older
  `CIM-schema-cimNN` namespaces; the writer always emits CIM100. cim18v17,
  and the IEC 61970-460 and -470 profiles that replace IEC 61968-13 CDPSM,
  are not implemented.
- **Application profile.** The single document feeder profile the GridAPPS-D
  and CIMHub tools exchange, with `ConnectivityNode` (node breaker)
  connectivity and per phase equipment. Format discovery
  (`powerio::grid_formats`) labels the `cim-dist` token
  "CIM distribution (CIM100 cim17v40, GridAPPS-D profile)".
- **Input.** One document, or the top level `.xml` documents of a directory,
  read through the same `Source` acquisition as every directory format: the
  `POWERIO_MAX_REFERENCED_BYTES` budget applies and symbolic links are
  refused. ZIP archives are not read; the bounded ZIP reader is still private
  to the CGMES reader.
- **Validation.** The CIM schema workflow checks fresh output for every
  committed distribution fixture against the cim17v40 vocabulary and the
  GridAPPS-D `rc4_2021` profile classes CIM-Graph generates
  (`evals/validation/cim_dist`); PowerIO output has no vocabulary errors.
  `evals/validation/validate_cim_tanks.py` compares the tank fixture with
  native OpenDSS. CIM-Graph round trips and cimhub-opendss comparisons are
  not run yet.
- **Not mapped.** Regulator and tap changer controls (`RegulatingControl`,
  `TapChangerControl`), distributed energy resources
  (`PowerElectronicsConnection`) and machines, wire and cable geometry,
  operational limits, and `LoadResponseCharacteristic` are reported on read
  (`READ.CIM_DIST.CLASS_UNMAPPED`, `READ.CIM_DIST.FIELD_UNMAPPED`). A
  document that connects equipment through `TopologicalNode` (bus branch)
  only is refused. On write, generators, inverter based resources, and
  control profiles are reported as dropped (`EMIT.CIM_DIST.RECORD_DROPPED`),
  as are conductors beyond A, B, C, and N (`EMIT.CIM_DIST.FIELD_DROPPED`).
  Because generators do not survive a CIM round trip, `cim-dist` is not yet
  in the conversion matrix.

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
the retained dependency buffers, even if the original files have since disappeared.
Unreferenced supplied buffers remain on the source module but are not copied to
the emitted project.
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
Names beginning with a digit or hyphen retain their IDs; canonical output enables
native relaxed naming when required.

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
`line_limits`, positive default voltage convergence tolerances and
`multi_recorder` objects are also retained and reported. Passive equipment
names resolve case-sensitively, including names differing only by case;
typed component names still need distinct case-insensitive identities.
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
physical delta export still requires its own mapping.

FBS `DELTA_DELTA` instead imports its native hybrid equations as a nine-terminal
admittance with two ideal external contacts. Three auxiliary voltages encode
scaled output currents: the native law removes common-mode input voltage but
transfers common-mode current. This preserves the pinned solver's behavior for
unbalanced grounded loads without substituting physical delta isolation.
Coordinates follow the equivalent's shunt and contacts; ratings remain
source-only and are diagnosed. Twelve native comparisons cover both forks,
step-up/down/equal voltages, and wye/delta loads through IR and OpenDSS.
OpenDSS writing preserves the full asymmetric admittance with coupled two-port
blocks and exact diagonal corrections. Internal negative-element losses are
not physical device losses; measure the aggregate external ports. Canonical
GLM writing of this auxiliary-node equivalent remains unsupported.

`DELTA_GWYE` (also numeric `3`) imports for NR and FBS as three single-phase
coils. Each primary joins two phase terminals; each secondary joins its phase
to ground. The native step-up and step-down orientations differ, including
their phase shifts. The mapping preserves that orientation and zero-sequence
blocking without depending on a target's default transformer clock angle.
Coil ratings use one third of total power and secondary coil voltage uses
line-to-neutral voltage. Both pinned engines and IR-restored OpenDSS agree
on unbalanced step-up, step-down, equal-rating and IEEE8500-parameter circuits.
Coordinates attach to all three coils. Canonical GLM groups a complete coherent
three-coil bank into one native `DELTA_GWYE` transformer. Its configuration uses
the current effective winding voltages, total rating and series impedance,
including edits after IR restoration. Diagnostics identify the grouped coil
names; coincident component points remain on the bank. Conflicting points fail.

The coils must share effective voltages, ratings and impedance, and their phase
pairs must match the native orientation for the current voltage ratio. Incomplete
or unequal banks, parallel banks at the same endpoints, grounded primary returns,
and ratio edits that cross the native orientation boundary require additional
mappings and fail explicitly. Component qualification covers step-up, step-down,
equal-voltage and IEEE8500 nameplate cases on both forks. Complete IEEE8500
canonical qualification is described below; the other bank restrictions remain.

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
FBS `WYE_WYE` regulators instead retain ideal zero-leakage winding constraints;
the native FBS solver ignores declared regulator resistance. OpenDSS emission
uses a one-ohm winding resistance followed by its exact negative, eliminating
both at an unloaded internal bus. This preserves the ideal ratio and external
currents without an arbitrary small leakage. A diagnostic identifies the added
line: physical secondary powers and total losses include it. The typed value
and IR retain zero leakage.

Canonical GLM now writes grounded ideal phase ratios as a series of native FBS
manual regulator stages. Each stage changes one phase and passes the other
connected bank phases at unity ratio. This avoids native current/loss errors
from parallel single-phase regulator objects. Ratios come from current effective
winding voltages; unrelated ratios do not need a common integer tap lattice.
Generated intermediate buses are internal to the equivalent, and diagnostics
identify the physical bank ports. Distinct component points remain on separate
stages. Declared winding ratings are diagnosed as omitted because native
regulator configurations have no power-rating field. Reimport absorbs redundant
unity coils without multiplying the stage count. Open-delta connections, banked initialization and floating-return GLM
writing remain unfinished.

Plain ideal closed switches likewise use cancelling resistances in OpenDSS;
plain open contacts are disabled and have no artificial line charging or leakage.
Switches retaining source-specific impedance declarations use the existing
source-aware path.
OpenDSS `Line` objects with `switch=yes` retain their finite line impedance and
fixed terminal states. Operating-point `switch_closed` columns address typed
ideal `DistSwitch` contacts. An application can add an explicit series contact
while retaining the imported finite branch; this supplies contact states without
executing an OpenDSS switching sequence.
 Transformer cancellation currently covers complete two-coil,
single-phase winding records with grounded or floating secondary returns and
no core or neutral shunts. A floating return uses two cancelling conductors,
preserving common-mode displacement. Other ideal-transformer profiles need
further qualification.

Canonical node convergence is set to `1e-9 V`. FBS output permits 1,000
iterations so larger edited feeders can reach that stopping precision. The tighter former absolute
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

Capacitor `VAR`, `VOLT`, `VARVOLT` and `CURRENT` controls import as fixed
snapshots only when every phase has an explicit switch state and control is
individual. `READ.GLM.CONTROL_FROZEN` reports excluded control execution.
Thresholds, sensing references and delays remain in source; canonical output
uses manual coils with the current typed admittances. Missing switch states and
unqualified banked initialization fail. A single-phase manual bank sensing its own phase is supported. An explicit `cap_nominal_voltage` sets its admittance base; otherwise native initialization uses the parent bus nominal voltage.

The IEEE8500 native reference harness compares its original PNNL input with
explicit static configurations on both forks. Arras requires `triplex_load` constant
power declarations where this older input uses `triplex_node.power_1/power_2`.
The harness also handles different dump-mode spellings. That initial dump-only
reference remains separate from full-precision import qualification.

The complete pinned IEEE8500 fixed-state case now has native-to-OpenDSS import
qualification through IR, across both forks. The full-feeder harness compares
all 4,879 native bus objects and 4,876 device ports, including center-tapped
services, triplex neutral currents and ideal regulators/switches. It verifies
physical powers and losses, native loss-report conventions, and current balance
at all non-source, non-ground bus terminals. The original PNNL input and portable
static input produce identical typed circuits. Native convergence is tightened
from a nominal-voltage fraction of `1e-4` to `1e-12` for comparison; electrical
data and the common comparison budgets are unchanged. A deliberately wrong
regulator tap must fail.

The pinned IEEE8500 case also qualifies fresh canonical GLM on both forks,
including IR-restored, rebased, explicitly grounded-source and typed edited
circuits. Reimported GLM and both DSS outputs agree at every original bus and
physical link port. Edited circuits change load powers, winding voltages/taps,
leakage and line lengths; their results are checked against the independent DSS
engine and bus current balance. The harness preserves all 82,901 electrical
observations per comparison. The ten engine/stage cases pass 38 comparisons
and 187,550 current-balance observations without relaxing comparison budgets.
This does not establish the remaining IEEE9500 or general OpenDSS profiles.

Grounded center-tapped services import as four typed windings and a
constant-impedance load at an auxiliary core bus. The fourth winding exposes the
internal core branch after primary leakage; the two secondary windings preserve
opposite polarity. Configuration phase ratings determine the impedance base.
Triplex loads retain separate leg and line-to-line P/I/Z terms. These circuits
have native-to-OpenDSS import qualification on both pinned forks, including
IEEE8500 service parameters and an IR round trip. The internal core load must be
included when aggregating physical transformer losses.

Triplex line geometry imports as a full three-conductor impedance matrix with
neutral grounded at both endpoints. This is equivalent to the native grounded
neutral reduction and retains its reconstructed neutral current. Unequal
conductors, 50/60 Hz, NR/FBS, mixed loads and connected IEEE8500 service parameters
have native-to-OpenDSS qualification, including both-end powers and losses.
Direct triplex 2×2 matrices also import. GridLAB-D's reversed leg-2 voltage
convention requires negating the second matrix row to obtain physical terminal
voltages and currents. The resulting matrix can be nonreciprocal; import preserves
it and reports `READ.GLM.REDUCED_TRIPLEX`. Such input does not define a physical
neutral/ground-current model. OpenDSS output uses an exact source-and-line
equivalent, including an open auxiliary third port, when needed. Aggregate its
physical end ports for powers and losses; internal element losses alone are not
the original line's losses. Both forks and solvers have direct-matrix microcase
qualification through IR and DSS, including deliberate wrong-load checks.
Floating service neutrals, service switches and additional geometry models
remain unfinished and fail explicitly. Grounded services can participate in a
checked FBS forest with ordinary-phase ideal contacts; the private core bus is
contracted for topology validation. Native checks cover impedance and mixed
P/I/Z services, IEEE8500 cable parameters and direct hot matrices through edits
and repeated output. Every contact port is included in these checks.

Canonical GLM can now write grounded center-tapped services from their current
four-winding circuit and private impedance core load. Effective winding ratios,
leakage and core admittance set the native configuration; retained source
parameters do not override edits. The core bus must have no additional device
or external connection. Unequal effective leg ratios require another mapping.
Triplex power and impedance terms keep separate load objects. Current terms
are combined on the root triplex_load so native NR updates their voltage angle
every iteration; diagnostics report the grouped identities. Distinct geographic
points on grouped current terms fail instead of being merged silently.

Full three-conductor triplex matrices emit a native geometry derived from the
current coefficients, with every primitive entry checked before writing. This
preserves grounded-neutral currents for that geometry family; it does not fit
arbitrary edited matrices or recover the original construction. Direct
hot-conductor matrices instead emit with the inverse leg-2 row convention and
report that no physical neutral-current model is inferred. Line, configuration
and transformer coordinates use the current typed points. These component
profiles have separate complete IEEE8500 qualification above. Floating or
arbitrary neutral circuits and unequal service leg ratios remain unsupported.

Nonzero triplex `base_power_1`, `_2`, and `_12` initialize separate constant-power,
constant-current and constant-impedance terms, overriding explicit coefficients
on that connection. Import reproduces native power-fraction normalization and
signed power factors, with `READ.GLM.LOAD_NORMALIZED`. Zero base power leaves
explicit P/I/Z coefficients in effect.

A missing or zero overhead mutual spacing produces zero mutual impedance in the
pinned engines. Import preserves this behavior and reports it instead of
inventing a spacing. Capacitance geometry still requires its qualified positive
dimensions. Seasonal conductor/configuration ampacities are validated and
retained in source with `READ.GLM.SOURCE_ONLY`; projection into typed ampacity
limits remains unfinished. Full OpenDSS
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
Conducting links must form a forest with one source bus per component. The check includes grouped ideal regulators,
delta-grounded-wye banks and grounded service transformers, with their private
core circuits contracted. Links are oriented away from their sources, and
disjoint switch phases between the same buses are grouped into one native
object. Endpoint reversal and grouped identities are reported. Matching
component points survive grouping; conflicting points fail explicitly.
Fully open ties between already connected endpoints use unloaded child nodes
parented to their physical endpoints. These aliases preserve both terminal
voltages, zero currents and losses, and the switch point. They avoid native
rank cycles and outgoing-current contamination; reimport recovers the original
bus attachments. Other open links retain their island attachment. Closed
cycles, parallel contacts on the same phase, connected multiple source buses
and bus phases absent from the incoming link need additional equivalents. No small impedance replaces an ideal
constraint, and the input network is unchanged.

For noncommuting line impedance and capacitance matrices, the pinned engines
use a source-end current equation that differs from the physical pi-line law.
NR uses it for reporting, so NR import retains the physical circuit and warns
about the reporting discrepancy. FBS uses it during its solve: FBS import now
preserves the native equations as a conductor admittance with auxiliary current
variables and ideal external contacts, reporting `READ.GLM.LINE_EQUIVALENT`.
Line coordinates follow the shunt and contacts; configuration coordinates and
calculated coefficients remain on the retained linecode. OpenDSS output
preserves this nonreciprocal circuit. Eight analytic/native/IR/DSS cases cover
two- and three-phase examples, and IEEE13 FBS matches both pinned engines.
IEEE34 still has two loss comparisons just above the strict tolerance; IEEE123's
zero-voltage configuration has one undeclared-phase voltage discrepancy. Neither is
claimed as a complete raw match. Canonical GLM writing of these native FBS
auxiliary-node equivalents remains unsupported.

For physical pi-line networks, canonical output replaces native line capacitance with exact phase-pair and
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

Finite scalar lines with impedance magnitude between `1e-8` and `1e-5` ohm
can emit a current-variable equivalent in NR GLM and OpenDSS. This profile
requires matching single-conductor endpoint maps, no line end shunts and no
unresolved electrical extras. It preserves the exact finite impedance; it does
not replace the line with an ideal contact or add an impedance floor.

The generated auxiliary voltage represents current with a one-ohm scale.
Physical terminal currents include the generated endpoint shunts, and physical
loss is the sum of signed terminal powers. Internal resistor losses are not
the original line loss. Emission diagnostics identify the equivalent elements
and report omitted thermal limits and routes. Reimport keeps the explicit
circuit; coupled matrices and more extreme impedances retain their existing
projection and are not covered by this qualification.

Canonical NR output sets `default_maximum_voltage_error` to `1e-9` per unit
and lets each node derive its absolute convergence limit from nominal voltage.
This avoids an absolute nanovolt stopping threshold on large feeders. FBS keeps
its previously qualified absolute node setting. These are engine stopping
criteria; the independent electrical comparison budgets remain separate.
Zero-admittance shunts emit zero-current loads with their current component
points and a class-rebasing diagnostic. No nonzero capacitor rating is invented.

Output without ideal closed contacts selects NR. The pinned engines can return NaN results for
some valid circuits even with a successful process exit. One qualified
impedance case reproduces this with either a native switch or its line
equivalent; FBS solves the same circuit and matches the analytic solution.
Engine validation therefore checks finite voltages, currents and powers, and
reports these NR failures separately from successful electrical comparisons.

Standalone `substation` objects can supply fixed zero-, positive- and
negative-sequence voltages. The reader applies the native `reference_phase`
rotation and keeps the resulting phase voltages. Nonzero sequences require an
explicit ABC SWING bus and override phase-voltage declarations. With all
sequences zero, ordinary node initialization applies. Parented substations and
external transmission coupling remain unsupported; standalone reporting settings
are retained with a diagnostic.

Static `series_reactor` objects map to uncoupled line impedances with unit
normalization length. Scalar resistance/reactance aliases apply in source order.
Zero impedance uses an ideal contact in FBS; NR uses the pinned native fallback
`1 / (10000 + j10000)` ohm. Mixed ideal and finite phases remain separate typed
components with their source coordinates. Current ratings remain in source with
a diagnostic; typed rating projection and open reactor states remain unfinished.
Canonical GLM output uses the existing line, switch and fixed-source equivalents.

PNNL 5.3 `diesel_dg` in `CONSTANT_PQ` mode reads as three single-phase
constant-power generators, retaining unequal signed P/Q, the initialized
per-phase rating limit, and component coordinates. Scalar power fields override
the complex fields at initialization; the native `-1` sentinel leaves that
component unchanged. Missing or nonpositive `Rated_VA` uses the pinned engine's
500 kVA default. Positive `Rated_V` must match the parent's line-line nominal
voltage within 1%. An ordinary ABC parent and `module generators;` are required.
Dynamic modes and Arras's different diesel model remain unsupported.

OpenDSS emission of generators without retained voltage bounds supplies a
0..2 pu constant-power band and reports the substitution. Above the emitted
upper bound OpenDSS uses constant impedance. Generators parsed from OpenDSS
retain their explicit bounds or native 0.9..1.1 pu defaults. Generator rating
limits remain in IR; their OpenDSS mapping is unfinished and reports a loss.
Canonical GLM writes current fixed generator injections as signed
constant-power loads, preserving phase/phase-pair connections and component
points. Reimport retains their electrical law under load identities. Ratings,
optimization bounds and costs have no matching load fields and are diagnosed
as omitted. Generator electrical extras or voltage/current laws outside this
profile remain unsupported. This device qualification does not qualify the
complete IEEE9500 feeder.

Solar-fed `FOUR_QUADRANT` inverters in `CONSTANT_PF` mode read as fixed
per-phase inverter injections, including native efficiency and rating clipping.
The supported solar profile is `BASEEFFICIENT` with default orientation and
static climate temperature and direct irradiance. The reader evaluates its
thermal equation; the inverter's declared `P_Out` does not set dispatch in this
mode. Split-phase inverters connect across the two hot terminals. OpenDSS
emission derives their line-line voltage base from winding polarity and
conductor mappings; retained OpenDSS voltage bases still take precedence.
Component coordinates remain attached to the typed inverter records.

Both pinned forks are checked on ordinary and split-phase circuits, including
the IEEE9500 solar parameters. Arras requires a meter or triplex-meter parent.
An internal `LI_ION` battery on this inverter profile initializes with zero DC
input and therefore zero AC injection, even if `P_Out` requests charging. Import
preserves that idle electrical state and reports `READ.GLM.BATTERY_IDLE`; energy,
state of charge and inactive load-following settings remain source-only. Native
checks cover both solvers and forks. Other battery operating modes, weather files,
multipoint efficiency remain unfinished. Canonical GLM writes explicitly fixed
inverter P/Q bounds as signed constant-power loads on the current ordinary or
split-phase connection. Available power alone is not dispatch; missing or
nonfixed bounds, active profiles and unresolved voltage/current laws fail.
Points remain attached to the equivalent, while prime-mover, availability and
rating metadata losses are diagnosed.
Climate coordinates and thermal inputs remain source-only with diagnostics.

OpenDSS rated wye reactors with kV/kvar and explicit R or Rp map to the
existing shunt admittance. R=0 retains the rating-based reactance; R does not
select the explicit-X specification mode. The profile preserves resistance,
parallel damping and the snapshot-frequency admittance. Canonical output uses
the current typed matrix, including edits after reimport. Copied state,
competing specifications, curves and delta resistance remain unsupported in
this profile. Native tests cover thirty phase/frequency/impedance cases.

OpenDSS Transformer Open/Close commands preserve fixed conductor states for
one-, two- and three-phase circuits. Open ports move to internal zero-current
terminals while retaining editable winding parameters; ideal contacts preserve
closed connections. The equivalent includes the native external diagonal
stabilization. A whole-terminal command changes phase conductors, leaving the
neutral state unchanged. Two-phase delta windings retain their third hot
terminal. Native checks cover 84 cases through IR, canonical DSS and reimport,
including three-winding units. Aggregate losses must use the external ports.
Layout, XfmrCode or copy changes after terminal commands, and disabled units
with terminal commands, remain unresolved. Canonical GLM mapping of these
circuits remains unfinished. Enabled unconverted Fault objects block electrical
emission until their static admittance and switching state are represented.

OpenDSS Generator model 6 delegates its voltage/current law to an external
library. The three-phase delta `IndMach012a` fixed-slip profile reads into the
existing full shunt admittance, including positive- and negative-sequence
behavior. Canonical DSS derives its equivalent circuit from the current typed
matrix. Native checks qualify both public four-wire lagging/leading feeders,
including IR, canonical reimport and all original motor terminals. A separate
27-case native sweep covers voltage, slip, initialization order and typed
matrix edits. Explicit positive kV/kVA bases and named numeric userdata are
required; library reloads, successive edits and slip-limit setter order are
preserved. Variable slip, copied hidden state, other connections/user libraries
and unsupported parameters remain blocked by
`READINESS.DSS.GENERATOR_USER_MODEL_UNRESOLVED`. Retained same-format source
emission remains available. No new network type or IR schema is needed.

OpenDSS Storage voltage model 1 reads fixed idling, charging and discharging
power as signed `BoundedConstantPower` loads. The profile covers one-phase wye
and two-/three-phase wye or delta, including reactive-only states. It preserves
voltage bounds, current limits, setter order and end-of-edit state-of-charge
guards. Canonical DSS derives fixed equivalents from the current typed powers,
including edits that mix consuming and generating branches. Energy integration
and dispatch remain source-only. Inverter curves, P/Q clipping, copied inverter
state and one-phase delta remain unsupported and block electrical readiness.

The PNNL NR profile accepts future manual reliability events only when their
start date is strictly after the complete clock window, all references resolve,
and every declared phase reaches a fixed source. Connected `SWING_PQ` buses in
the grid-association profile remain PQ buses, as in native initialization. Island
promotion, active outages and protection execution remain unsupported. These
choices produce explicit static-state diagnostics. Per-phase link current
ratings are validated and retained in source; typed ampacity mapping is pending.

Both original CIMHub IEEE9500 balanced and unbalanced projects now parse through
IR and produce solvable OpenDSS circuits with their DER and fixed device states.
Both projects also produce canonical GLM from their current typed fixed
injections and passive networks. This is output availability, not full-feeder
qualification of canonical GLM. Both complete cases meet the documented
one-part-per-million feeder accuracy profile for all 84,501 original voltage,
current, power and loss measurements through import, IR and repeated DSS output.
Independent external-terminal current balance also passes. The tighter analytic
calibration comparisons remain separate and retain their discrepancies, including
native NR line-current reporting when impedance and shunt matrices do not commute.
Reimported disabled contacts retain inactive-state metadata; canonical DSS
reports their omission, with zero external currents and endpoint voltages still
checked. Physical terminal powers use endpoint voltages and boundary currents,
not internal equivalent-source powers. Fresh canonical GLM now solves both
complete cases on both pinned forks with the same feeder accuracy and physical
current-balance checks. An edit that introduces source zero-sequence voltage
exposes a native NR delta-grounded-wye primary-stamp discrepancy. NR input
containing these banks and canonical NR output therefore refuse nonzero
zero-sequence voltages at complete three-phase fixed sources. Balanced edits
remain supported, and the FBS reader keeps its separate mapping. This is an
explicit profile restriction, not a numerical correction or tolerance change. Broader format compatibility remains unfinished.

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

The supported schemas ship in the Rust crate at `schemas/bmopf/`, in the
Python package at `powerio/schemas/bmopf/`, and in each C/Julia library archive
at `schemas/bmopf/`. Their manifests record the exact bytes and upstream
review references. Python can read them with `importlib.resources.files("powerio")`.
Upstream review changes do not alter an installed release's schema. A new
schema interpretation requires a reviewed PowerIO release; a PowerIO release
does not establish Task Force acceptance.

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
freezing named controls. Bounded ZIP, exponential and constant-power laws can
be projected only when nominal voltage lies strictly inside a smooth law
region, above dropout and below any current-limiting transition. The action
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
in PowerIO. The LinDist3Flow OPF types require PowerIO 0.11.1 and the
fixed-dispatch types 0.11.4; existing saved IR 2 networks require no format
migration.
