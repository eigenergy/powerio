# MATPOWER AC/DC equivalents

PowerIO reads the baseline PowerModelsACDC/PGLib HVDC MATPOWER profile into
`BalancedNetwork::ac_dc_equivalent()`. Use automatic detection or the explicit
`matpower-acdc` token. An explicit ordinary `matpower` request refuses an
extended case. The parser shares the MATPOWER lexer and ordinary AC reader.

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
and the full converter station preparation are subsequent work. Existing
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

The pinned case5 fixture and `evals/validation/acdc` harness compare every
AC/DC declaration and a fresh-written file with PowerModelsACDC's underlying
parser. Source declarations and reference preprocessing results are saved
separately. This establishes IO equivalence, not OPF feasibility or objective
agreement. Tellegen's full-station ACP benchmark is separate work.
