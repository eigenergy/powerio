# PowerModelsACDC MATPOWER extension

PowerIO reads the MATPOWER-extension variant of PowerModelsACDC, including
the baseline PGLib HVDC profile, into
`BalancedNetwork::ac_dc_equivalent()`. Use automatic detection or the explicit
`matpower-acdc` token. An explicit ordinary `matpower` request refuses an
extended case. The parser shares the MATPOWER lexer and ordinary AC reader.

The native JSON variant, based on serialization of the PowerModelsACDC IR
(data dictionaries), is a separate future adapter. It is not implemented by
this profile. Both source formats are distinct from PowerIO's own IR, which
already preserves the imported typed equipment.

The profile accepts `busdc`/`dcbus`, `convdc`/`dcconv`, and
`branchdc`/`dcbranch`. It requires an explicit `dcpol` of 1 or 2 and complete
nonempty topology declarations; `branchdc = []` supports back-to-back stations.
DC buses have eight columns, or nine with the documented `area` header;
branches have nine and converters 34. Headers, when present, must describe
the supported layout. Aliases cannot be supplied twice. All numeric fields
must be finite, flags integral 0/1, and references valid even on disabled
records. DC external IDs are positive 32-bit integers in a separate namespace
from AC bus IDs. Source `grid` numbers are metadata, not partitions.

Fresh output uses canonical names and column headers and works after IR
restoration. Unchanged same-profile output echoes the source bytes. An edit
invalidates the retained source through the module's existing edit API. Other output formats
refuse the equipment until an explicit qualified projection is supplied;
the writer cannot replace a meshed grid by independently editable `dcline`s.
Ordinary MATPOWER and aggregate HVDC conventions are unchanged.

## Electrical declarations

The optional COW owner contains DC buses, branches and converter stations.
The power base and pole multiplier belong to the equivalent, which does not
claim to describe physical grounding or return conductors. DC inductance and
capacitance retain their declared based units. A/B/C DC ratings are endpoint
MW limits: zero is zero, not the AC MATPOWER unlimited sentinel.

Converter impedance/filter parameters retain their source AC per-unit
coordinates and the independent converter voltage base. This reader does not
infer an ohmic conversion from the PCC bus base. Disabled transformer, filter
and reactor parameters, all control settings, and both directional loss
coefficients survive IO. LossB/LossC retain the source AC-current conventions;
they are not interpreted as DC-current loss coefficients.

PCC P/Q settings use passive withdrawal, so P_g and Q_g are negated. Valve-side
P/Q capability bounds and the passive DC target Pdcset are retained without
negation. AC target Vtar remains distinct from the bus voltage and numerical
starts. Droop remains the declared pu-voltage/pu-power coefficient; no MW slope
is silently substituted. The importer does not enlarge Imax or select one
loss coefficient. Those are explicit calculation preparation policies.

## Capability and persistence

Typed Rust import, editing, IR and fresh export are supported. Coupled PF/OPF
solvers remain external to PowerIO; the explicit OPF interface below supplies
full coupled preparation. Coupled PF interfaces remain subsequent work. Existing
PF/OPF constructors, normalization and AC-only numerical preparation refuse
nonempty equivalent equipment before generator-only supply filtering.
Automatic island-reference assignment reports an error and leaves it unchanged.
AC graph inspection still describes the AC graph, not DC connectivity.

Clone/COW preserves the owner. Bus remapping updates converter PCC references.
An explicit AC subset keeps the DC buses and branches and removes stations
whose PCC is outside the retained AC buses; it does not establish DC
feasibility. Explicit de-energization disables affected stations. Automatic
pass-through reduction preserves station buses. Fresh export refuses different
AC/equivalent MVA bases until a qualified rebasing operation is available.

IR stays version 2. Extended values carry the top-level requirement
`balanced-acdc-equivalent-v1`, including disabled equipment and nested
operating points, time series and scenario sets. The reader checks the actual
value against the requirement, and refuses missing, extra or unknown
capabilities. Ordinary documents keep their historical shape. Published
0.11.0 and 0.11.9 readers reject this top-level field; without it they silently
discard new electrical fields. Published schemas are preserved; the development
schema is under `pio-ir/2/unreleased` pending release selection.

No new C ABI symbols or Julia wrapper release is included. Existing parse/IR
transport surfaces retain the value, but typed C/Python/Julia equipment access
and coupled calculation support are not claimed by this Rust IO milestone.
TNEP, switching, additional electrical tables, type-4 droop requiring Pacset,
and unqualified layouts are refused. LCC declarations are retained; no LCC
calculation is implied.

## Validation

The pinned `evals/validation/acdc` harness checks the whole electrical case,
including AC topology, generators, costs, limits, taps and statuses, after
PowerIO IR restoration and fresh MATPOWER-extension output. File provenance
and previous solve duals are excluded explicitly and recorded; electrical
input fields are not excluded. Source and effective preprocessing data are
saved separately.

The harness also solves original and reconstructed cases with the same pinned
PowerModelsACDC ACP formulation and Ipopt settings. It checks feasibility,
objective agreement, and each solved point against the other model's constraints
and variable bounds. PF states, converter P/Q, DC flows and losses are compared.
OPF state differences are recorded; multiple optima need not have identical
dispatch. These checks qualify the importer/exporter, not a PowerIO or Tellegen
coupled solver. The native-equation and parameter-variation work is recorded in
`evals/validation/acdc/FOLLOW_UP.md` for mathematical-model development.

## Coupled OPF interface

`AcDcOpfInstance::from_network(network, profile)` requires the explicit
profile selection. `PowerModelsAcDc0101AcpVscV1` means ACP VSC,
network generator cost, all operating limits, and free generator/converter
OPF dispatch. Retained PF controls and targets do not constrain that dispatch.
The source network remains immutable through the instance. The initial point
is a numerical guess and may violate the equations.

`AcDcOperatingPoint` contains a complete set of `AcDcValue` records keyed by
`AcDcQuantity` and stable component identity. Powers use MW/MVAr, angles radians,
voltages pu in source coordinates, and valve current pu on the station AC base.
Powers enter equipment terminals; generator powers are production. Valve power
enters the valve from the reactor. Constructors refuse incomplete, unknown,
duplicate and nonfinite coordinates. Row order is not identity.

`build_ac_dc_opf_preparation` in `powerio-matrix` supplies the AC arrays and all
DC/station blocks. Consumers must assemble every block. Preparation scales
powers to the common MVA base; source declarations stay unchanged. It records
both declared and effective Imax, converter and PCC bases, directional loss
coefficients, and effective bounds. The pinned reference policy uses LossCinv
in both directions, widens Imax to the P/Q capability box radius, and applies
its auxiliary port/voltage bounds. Loss scaling is
`a=LossA/S`, `b=LossB/(sqrt(3)*V)`, `c=LossCinv*S/(3*V^2)` with the converter's
AC voltage base. This is compatibility with PowerModelsACDC 0.10.1 commit
`9ada4082ac087aed649f48fb5f977680b0fe06c7`, not physical rebasing to the PCC.

`AcDcOpfSolution` shares its owning instance and contains the typed primal point,
objective, termination and producer. Construction validates shape and finite
values, not equation feasibility. The producer must validate its solution;
`Converged` does not claim global optimality. Solver tapes and unqualified duals
are absent. Serialize these through `PioValue::AcDcOpfInstance` and
`PioValue::AcDcOpfSolution`: both are new IR 2 structural types, require the
AC/DC capability, and reject unknown profiles/fields. Published schema snapshots
are unchanged. Existing AC-only constructors retain their rejection guards.

This initial interface excludes normalized/base-mismatched data, LCC, mixed
physical/aggregate DC, AC self loops, storage and three-winding transformers.
Preparation qualifies polynomial generator costs; piecewise costs need a
separate auxiliary-coordinate qualification. General bindings, collection
variants, native coupled PF, additional controls, physical projections and the
natural PowerModelsACDC JSON variant remain future work. No new C ABI symbols
or Python/Julia calculation modes are introduced.

### Numerical OPF angle references

Coupled intent defaults to `AcDcAngleReferencePolicy::Declared`, retaining the
reference-coverage requirement. Opt into `LowestBusIdPerIsland` with
`with_angle_reference_policy` to add zero-radian numerical anchors only to active
AC islands without a declared reference. The lowest source bus ID is selected;
in-service AC branches and closed switches define connectivity, not DC links.
Every declared reference is retained. Generator availability does not determine
this numerical choice or add PF dispatch/voltage controls.

`AcDcOpfPreparation::angle_references` records each anchor, its sorted component
bus IDs, and whether it was declared. The private AC assembly projection uses
that reference set; the input network and source bus roles are unchanged.
Ordinary AC-only constructors/preparation retain their existing guards.
The policy persists with coupled instances/results; old records without the
field retain the strict default. The unreleased schema documents the new field.
Consumers must explicitly align reference points by shifting all bus and
converter filter/valve angles in the island together and checking finite angle
bounds. Preparation does not silently rotate stored operating points.

### Physical local-coordinate profile

`PhysicalCoordinatesAcpVscV1` derives `PhysicalLocalBases` coordinate policy
and the declared-current policy. Both existing profiles retain their source
per-unit equations and current policies. This third selection is an explicit
interpretation of the supplied owner, not a general rebase or import conversion.
All active AC buses must declare finite positive voltage bases.

The PCC uses its AC bus line-to-line kV base; filter, valve, transformer and
reactor impedance bases use the station's `ac_base_kv`. Series impedances use
`Zbase = V_converter²/S`. An enabled transformer tap is the physical voltage
turns ratio times `V_converter/V_PCC`. A disabled transformer ignores retained
tap and impedance data and requires `V_PCC * vm_PCC = V_converter * vm_filter`.
The filter susceptance and declared RMS current remain on the converter base.

DC node bases are per-pole kV. Each branch's per-unit resistance uses its
**from-node** base: `R_ohm = r_pu * V_from_base²/S`. With
`k = V_to_base/V_from_base`, its terminal injections are
`pf = n/r * vf*(vf-k*vt)` and `pt = n/r * (k*vt)*(k*vt-vf)`.
Reversing branch orientation therefore also requires converting resistance to
the new from base. Preparation records port/impedance bases, physical turns,
resistance reference side and voltage ratios; nonrepresentable ratios refuse.

LossCinv in both directions and the reference auxiliary bounds remain explicit
limitations. No profile infers a physical pole/return topology. The new required
profile token survives instance/result IR; older decoders reject it. Consumers
must implement the recorded coordinate policy across every equipment block.
