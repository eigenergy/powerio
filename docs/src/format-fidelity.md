# Formats and fidelity

The [format table](https://github.com/eigenergy/powerio#formats) in the
repository README lists each format with its token and its read and write
support. This page covers the numeric conventions, the independent checks each
reader and writer passes, and, format by format, what a reader keeps and what a
writer reports.

## Conventions

PowerIO's numeric conventions follow MATPOWER and PowerModels.jl. For each
quantity the table gives the reference implementation and the matching PowerIO
code:

| Quantity | Convention | Reference | powerio |
| --- | --- | --- | --- |
| Bus type codes | \\(1 = \mathrm{PQ}\\), \\(2 = \mathrm{PV}\\), \\(3 = \mathrm{ref}\\), \\(4 = \mathrm{isolated}\\) | MATPOWER `idx_bus` | `network::BusType` |
| Impedance, susceptance | per unit on `baseMVA`, never rescaled | MATPOWER `idx_brch` (`BR_B` already per unit) | `matpower` |
| Branch terminal admittance | MATPOWER `BR_B` splits half to each end; richer sources use canonical `g_fr`/`b_fr`/`g_to`/`b_to`; one-value targets receive the total susceptance projection | PowerModels `matpower.jl`; MATPOWER `idx_brch` | `network::BranchCharging`, `Branch::calc_terminal_charging` |
| Tap ratio | `0` means a line (treated as `1`); nonzero is a transformer | MATPOWER `idx_brch` `TAP` | `Branch::calc_effective_tap` |
| Phase shift, angle | degrees in the model; PowerModels JSON carries radians | PowerModels `make_per_unit!` | `powermodels-json` |
| Angle limits | `angmin`/`angmax` default ±360 (unconstrained) | MATPOWER `idx_brch` `ANGMIN`/`ANGMAX` | `Branch::has_angle_limits` |
| pandapower/PyPSA impedance | line `r/x` are converted between per unit and ohms with \\(Z_{\mathrm{base}} = V_{\mathrm{kV}}^2 / \mathrm{baseMVA}\\); pandapower line charging is capacitance per km (`c_nf_per_km`, converted via \\(2\pi f \ell Z_{\mathrm{base}}\\)); PyPSA line `b` is siemens | pandapower PPC conversion, PyPSA static components | `pandapower-json`, `pypsa-csv` |
| dcline `Pt`/`Qf`/`Qt` | sign flips vs MATPOWER | PowerModels `matpower.jl` | `powermodels-json` |
| Generator cost | \\(c_2 p^2 + c_1 p\\) maps to \\(q = 2c_2\\), \\(c = c_1\\); coefficients high order first | MATPOWER `idx_cost`, egret `matpower_parser` | `GenCost::calc_quadratic` |
| `source_id` | `["bus", id]` for bus-tied elements | PowerModels `matpower.jl` | `powermodels-json` |
| PSLF shunts | EPC `pu_mw`/`pu_mvar` are per unit on `sbase`; `Shunt` stores MW/MVAr at \\(V = 1\\) | paired EPC/RAW case checks | `pslf` |
| DOE GO Challenge 3 | an input/problem data file parses to `AcScucInstance`; one `Source` containing that file and its matching output/solution data file parses to `AcScucSolution`; `instance.network()` returns the shared `BalancedNetwork` | pinned GO-3 data model, C3DataUtilities, and GOC3Benchmark.jl D1/D2/D3 files | `powerio::parse`, `powerio::emit` |
| Surge angles | Surge JSON carries voltage angles, phase shifts, and angle limits in radians; `BalancedNetwork` stores degrees | Rust Surge round trip tests | `surge-json` |
| DeepMind OPFData JSON | DeepMind OPFData carries p.u. powers and radian angles; `BalancedNetwork` stores the solved snapshot in MW/MVAr and degrees, with zero based links mapped to one based bus IDs | Paper Appendix A, the PyG loader, the smallest complete official fixture, and size independent FullTop and N-1 property tests | `opfdata-json` |
| UCTE-DEF units and signs | ohm, microsiemens, kV, MW, MVAr, and ampere on the node voltage level; generation and its limits are negative for an injection and the reader negates them; a current limit becomes `rate_a` as \\(\sqrt{3}\, U I / 1000\\) MVA; no system base, so the balanced view uses 100 MVA at 50 Hz | PowSybl Core [`UcteImporter`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ucte/ucte-converter/src/main/java/com/powsybl/ucte/converter/UcteImporter.java) and [`UcteNode.fix`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ucte/ucte-network/src/main/java/com/powsybl/ucte/network/UcteNode.java#L413-L444) | `ucte` |
| IEEE CDF shunts and taps | bus `G`/`B` are per unit on the title card MVA base and `Shunt` stores MW/MVAr at \\(V = 1\\); the tap bus is the from bus and the final turns ratio is the MATPOWER `TAP`; the phase shifter angle keeps its sign | MATPOWER `cdf2mpc`, PowSybl [`IeeeCdfBusReader`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ieee-cdf/ieee-cdf-model/src/main/java/com/powsybl/ieeecdf/model/reader/IeeeCdfBusReader.java#L26-L51) and [`IeeeCdfBranchReader`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ieee-cdf/ieee-cdf-model/src/main/java/com/powsybl/ieeecdf/model/reader/IeeeCdfBranchReader.java#L27-L62), the vendored 14 and 30 bus cases against `case14.m` and `case30.m` | `ieee-cdf` |

egret's own MATPOWER parser uses the same reductions (bus type as
`matpower_bustype`, polynomial coefficients reversed to a `{degree: coefficient}`
map, piecewise to `[[mw, cost], ...]`, impedances left per unit), which is why a
MATPOWER case taken through powerio to egret JSON matches egret's direct import.

## Validation

The harness script `evals/validation/run_validation.sh` checks powerio against
five independent tools, and each classic text reader and writer runs under an
oracle. The conversion matrix covers MATPOWER, PSS/E, and egret sources against
all five legacy text targets, each PowerWorld output is read back and bridged
to PowerModels JSON, and the PMread leg covers the PowerModels JSON read side.
pandapower JSON and PyPSA CSV folders have dedicated import validators, because
pandapower has its own JSON schema and PyPSA is a directory format; both
validate the write direction only, since the pandapower JSON and PyPSA readers
have no external oracle. DOE GO Challenge 3 has a separate pinned reference
job, described below. Surge JSON and the remaining source and target pairs
(PowerModels JSON and PowerWorld sources into the targets other than
PowerModels) rest on the Rust round trip suite.

- **PowerModels.jl** (`validate_powermodels.jl`, `validate_psse.jl`,
  `core_json.jl`) reads MATPOWER, PowerModels JSON, and PSS/E. The MATPOWER to
  PowerModels JSON path is checked field by field after per unit normalization,
  and the others by element counts and demand, generation, and shunt totals.
- **egret** (`validate_egret.py`) is the oracle for egret output, which
  PowerModels cannot read. It loads powerio's egret JSON with
  `egret.data.model_data.ModelData` and compares counts, totals, and generator
  cost curves.
- **ExaPowerIO.jl** (`validate_exapowerio.jl`) reads MATPOWER through powerio's
  C ABI and compares value for value.
- **pandapower** (`validate_pandapower.py`, `validate_pandapower_converter.py`)
  cross checks the MATPOWER parse and \\(Y_{\mathrm{bus}}\\), then imports
  powerio's pandapower JSON output back into pandapower and compares counts and
  \\(Y_{\mathrm{bus}}\\).
- **PyPSA** (`validate_pypsa.py`) imports powerio's PyPSA CSV folder output and
  checks counts, totals, line r/x/b rebased from ohms on the bus0 voltage, and
  transformer r/x/tap_ratio/s_nom rebased from the transformer `s_nom` base. A
  mismatch in the line and transformer split fails the case.

### The conversion matrix

`evals/validation/validate_matrix.py` converts each source to each legacy text
target and checks the electrical core of the output (bus, branch, and generator
counts and the per unit demand, generation, and shunt totals) against the
source's own core as an independent oracle reads it. The diagonal is checked
byte exact, meaning that writing a case back to its own format reproduces the
file. Sources are the real native files where they exist (the vendored PSS/E
`.raw` and egret `.json`) and representative MATPOWER cases otherwise: basic
(`case9`), shunts and transformers (`case14`, `case30`), size (`case118`,
`case2869pegase`), HVDC with a mixed piecewise and polynomial gencost
(`t_case9_dcline`), and a piecewise cost case (`pglib_opf_case5_pjm`).

All 65 legacy text cells pass (13 source cases × 5 targets). Each writer
preserves the core regardless of fidelity tier, which is why the core is the
invariant checked across the whole matrix; cost, HVDC, and angle limits are
tier specific and are covered by the dedicated checks above and the Rust suite.
The pandapower JSON and PyPSA CSV validators run alongside this matrix and are
reported as separate legs.

### Running it

```sh
cargo build --release -p powerio-capi
python3.11 -m venv .venv
.venv/bin/python -m pip install --upgrade pip maturin -r evals/validation/requirements.txt
env VIRTUAL_ENV=$PWD/.venv .venv/bin/maturin develop --release
julia --project=evals/validation -e 'using Pkg; Pkg.instantiate()'
bash evals/validation/run_validation.sh
```

The oracle tools (PowerModels.jl, egret, ExaPowerIO.jl, pandapower, PyPSA) are
declared only in `evals/validation/Project.toml` and
`evals/validation/requirements.txt`, and no PowerIO release artifact depends on
them. `evals/validation/run_validation.sh` expects the Python oracles to import
in the selected Python 3.11+ environment, so a missing PyPSA, pandapower, or
egret import is a setup failure.

## Format notes

Each loss produces a coded diagnostic. A reader itemizes what it keeps only in
the retained source, with the table and the count of affected rows; a writer
reports what the target cannot represent, and `emit` returns those findings
with the result. A code spells out the format and the reason, as in
`READ.CGMES.RECORD_UNMAPPED`, `READ.CGMES.FIELD_UNMAPPED`,
`READ.XIIDM.FIELD_UNMAPPED`, or `EMIT.PSSE.FIELD_DROPPED` for RAW and RAWX, so
there is no generic parse warning that hides the cause.

### XIIDM and JIIDM

PowerIO reads PowSybl IIDM 1.0 through 1.17 in the XML (`xiidm`) and JSON
(`jiidm`) encodings and writes 1.17 in either; the
[IIDM versions](#iidm-versions) table lists what each version changed. One
element mapping serves both encodings, so a JIIDM document reads to the same
network as the XIIDM document of the same network, and fresh JIIDM follows
PowSybl's JSON layout: plural array fields for repeated elements, typed
scalars, and each element's attributes in the order PowSybl's sequential JSON
reader consumes them.

The mapping covers substations, voltage levels, bus breaker and node breaker
topology, busbar sections, switches, lines, tie and boundary lines, loads,
generators, batteries, shunts, static VAR compensators, two and three winding
transformers, tap changers and controls, operational limits, reactive limits,
HVDC converters, aliases, properties, and PowSybl active power control. Areas,
one level of nested XIIDM networks, nonlinear shunt section models, physical DC
equipment, and VSC/LCC converters also parse and survive fresh emission. An
unknown extension subtree stays available for byte exact same format emission
and produces a diagnostic, but it does not become an unnamed field on the
network. Fresh emission preserves detailed connectivity when the source has it
and allocates any missing local node numbers without changing stable PowerIO
identities. A three winding transformer whose `ratedU0` differs from `ratedU1`
keeps that leg impedance base for fresh emission.

XIIDM gives electrical quantities in physical units and has no system MVA base,
so the balanced calculation view uses 100 MVA as its internal normalization and
does not report a missing source value. Fresh XIIDM or JIIDM emission reports a
network base other than 100 MVA, because the target has nowhere to put that
normalization; impedance and admittance conversion still uses the network's
actual base, so the physical electrical values come out unchanged.

### CGMES

PowerIO reads CGMES 2.4.15 on CIM16 and CGMES 3.0 on CIM100. CGMES 2.4.15 uses
the namespace `http://iec.ch/TC57/2013/CIM-schema-cim16#` with the ENTSO-E
extension namespace `http://entsoe.eu/CIM/SchemaExtension/3/1#` (IEC TS
61970-600-1/-2:2017); CGMES 3.0 uses `http://iec.ch/TC57/CIM100#` and
`http://iec.ch/TC57/CIM100-European#` (IEC 61970-600-1/-2:2021). Both use the
IEC 61970-552 CIMXML instance syntax, in which `rdf:ID` defines a record,
`rdf:about` extends one, and `md:FullModel` heads each profile document. EQ is
required, and SSH, SV, and boundary profile data are used when present. SSH
assignments take precedence over SV observations, so an SV shunt section count
that differs from the SSH assignment is reported and not kept. A set with
TopologicalNode data reads one bus per `TopologicalNode`, and that data wins
even for node breaker equipment, so source TopologicalNode identities survive.

A set without TopologicalNode data still reads when its declared profile URIs
describe node breaker equipment, using the same test PowSybl makes in
[`CgmesModelTripleStore.computeIsNodeBreaker`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/cgmes/cgmes-model/src/main/java/com/powsybl/cgmes/model/triplestore/CgmesModelTripleStore.java#L168-L184):
each CGMES 2.4.15 document that declares EquipmentCore also declares
EquipmentOperation (and each EquipmentBoundary document declares
EquipmentBoundaryOperation), or the EQ is CGMES 3.0 CoreEquipment with
ConnectivityNode records. The buses are then the connected components of the
ConnectivityNode graph joined by switches that are closed and in service, which
is the graph PowSybl's
[`NodeMapping`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/cgmes/cgmes-conversion/src/main/java/com/powsybl/cgmes/conversion/NodeMapping.java)
and
[`SwitchConversion`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/cgmes/cgmes-conversion/src/main/java/com/powsybl/cgmes/conversion/elements/SwitchConversion.java)
hand to IIDM.

A switch is open when SSH `Switch.open` says so, otherwise when EQ
`Switch.normalOpen` says so, and otherwise closed. SV `SvStatus.inService`, or
failing that SSH `Equipment.inService`, decides service status, which CGMES
defines as availability for topology processing (PowSybl 7.3 reads only the
switch position, and since the official sets contain no closed switch out of
service, both rules agree there). A terminal is connected unless SSH
`ACDCTerminal.connected` is false; a disconnected terminal leaves its equipment
on the bus of its ConnectivityNode with the equipment out of service, which is
where PowSybl inserts a fictitious open switch. Each bus takes the nominal
voltage of its nodes' VoltageLevel (a Bay resolves to its VoltageLevel), a node
in a Line container takes the base voltage of attached conducting equipment or
transformer ends, and a node no terminal references gets no bus. A bus is named
after a BusbarSection on it, or else after its first ConnectivityNode. Its mRID
is the UUIDv5, under PowerIO's CGMES namespace, of the sorted ConnectivityNode
mRIDs it joins, so the same nodes always yield the same mRID and no source
TopologicalNode mRID is invented; the `bus_breaker_buses` table stays empty and
`calculated_buses` lists the nodes of each bus.
`READ.CGMES.TOPOLOGY_CALCULATED`, a remark, gives the bus, node, and switch
counts and the identity rule; `READ.CGMES.CONNECTIVITY_INSUFFICIENT`, an error,
says which data is missing when a set has neither TopologicalNode records nor
calculable connectivity, such as a bus branch EQ without TP.

The calculated topology has limits. SvVoltage observations reference
TopologicalNodes, so bus voltages keep their defaults and
`READ.CGMES.RECORD_UNMAPPED` counts the observations. No TopologicalIsland
supplies an angle reference, so the reference bus comes from
`referencePriority`, an external injection, or the largest machine. A closed
switch between two voltage levels joins its nodes into one bus placed in the
first node's level, as PowSybl merges those levels. A 2.4.15 junction terminal
in EQ_BD has no ConnectivityNode and is read as disconnected when there is no
TP_BD. Finally, PowSybl's bus view omits a component with no busbar section and
fewer than two feeders while PowerIO keeps each component as a bus, so bus
counts differ by those components. Each document's `Model.modelingAuthoritySet`
is read for the boundary and state variable authority checks, reported, and not
kept; fresh output writes PowerIO's own modeling authority.

A source can be an XML profile directory, a directory of profile ZIP files, or
one ZIP containing the profiles. The mapping covers hierarchy, AC and DC
equipment, detailed connectivity, current, active power, and apparent power
operating limits, tap changers and controls, reactive limits, and the operating
and solution values in SSH and SV. Diagram, geography, dynamics, and
unrecognized CIM classes are counted in diagnostics. A field on a recognized
class that the mapping does not consume gets a grouped
`READ.CGMES.FIELD_UNMAPPED` diagnostic with the class, field, count, and sample
identities.

Fresh output is a deterministic CGMES 3.0 EQ, TP, SSH, and SV profile set.
Imported UUID mRIDs survive for mapped equipment, terminals, tap changers,
operational limit sets, hierarchy, and topology records, and a missing mRID
becomes a UUIDv5 derived from the component type and stable identity. Tap
changer tables, tap controls and table points, reactive curve points, and
individual limit values keep their electrical values and relationships but get
deterministic subordinate mRIDs on fresh emission. Operational limit helper
objects that PowerIO generates do not become source metadata on readback. Limit
type objects keep the PATL or TATL name common CGMES importers require, while
generated limit set objects omit an unrepresented display name. A third party
subordinate identity or field without a typed record still gets a diagnostic.
The source neutral limit model keeps permanent and temporary limits, so fresh
output uses PATL and TATL; parsing reports a PATLT or TCT substitution and any
fractional duration rounded to whole seconds.

CGMES input keeps each source `Substation`, including distinct substations
joined by a transformer. XIIDM and JIIDM emission joins output container groups
only when the IIDM rule that a transformer belongs to one substation requires
it, and it reports that hierarchy change; the transformer stays a transformer
with the same electrical data. Distinct `BaseVoltage` identities at the same kV
also get a precise collapse diagnostic before fresh output uses one record
keyed by voltage. Busbar `VoltageLimit` records are combined with the enclosing
voltage level into the most restrictive valid low and high voltage range; an
inconsistent pair is diagnosed and ignored, and fresh emission writes the
resulting `VoltageLevel` fields instead of recreating the individual
`VoltageLimit` records. CGMES uses physical units and has no system MVA base,
so the balanced calculation view uses 100 MVA as its internal normalization and
does not report a missing source value.

### PSS/E

PowerIO reads RAW revisions 32 through 35 and RAWX revision 35, and RAW and
RAWX share one electrical mapping. RAW 32 records end before the bus voltage
limits (`NVHI`, `NVLO`, `EVHI`, `EVLO`), the load `INTRPT` field, the
transformer `VECGRP` field, and the winding `CNXA` field that revision 33
added, so the reader lays each record out by the header revision, defaults
those fields, and reports a revision 32 record that ends before its last typed
field as `READ.PSSE.VALUE_DEFAULTED` with the record's byte range. RAW 34 maps
its substation section. RAW 35 and RAWX 35 map and freshly emit substations,
nodes, switches, busbar sections, and equipment terminal references. Fresh RAW
34/35 and RAWX output preserves AC line and transformer names, and RAWX
terminal rows use the exact type, buses, and local identifier chosen for their
electrical equipment row. When a source neutral connectivity node has no PSS/E
number, fresh RAWX allocates a positive number within its substation before
resolving exact regulation targets, and reports the default.

An explicit RAW revision outside 32 through 35, an invalid system base or
frequency, or a nonfinite record value is rejected. Fresh output accepts only
revisions 33 through 35 and returns an error when detailed connectivity cannot
form valid RAWX tables. A RAW 32 module keeps its source text like any other
revision, but no emission target names revision 32, so writing it back as PSS/E
produces fresh revision 33 text and its unmodeled sections survive only in the
retained source.

Generator `IREG`/`NREG`, switched shunt `SWREG`/`NREG`, and transformer
`CONT`/`NODE` resolve to exact terminal references, including an explicit
target on the same bus. Each winding of a three winding transformer keeps its
control mode, regulated terminal, limits, tap position and range, and number of
tap positions. A positive `COD` enables automatic adjustment, a negative `COD`
keeps the same mode with automatic adjustment disabled, and zero is fixed;
`|COD| = 4` controls a DC line quantity on a two winding transformer, and
`|COD| = 5` controls asymmetric active power flow. Unsupported RAWX tables,
including multiterminal DC, FACTS, GNE, induction machine, multisection line,
zone, owner, and interarea transfer records, remain only in byte exact same
format emission and produce counted diagnostics. Unknown RAWX tables and
`caseid` fields get the same retained source diagnostic, and fresh output
diagnoses detailed records and fields that RAWX cannot carry. Three winding
transformers are kept as typed records, and the indexed view lowers each one as
a star into \\(Y_{\mathrm{bus}}\\)/connectivity; two terminal DC lines map to
the neutral HVDC model. A switched shunt keeps its steady state susceptance
`BINIT` as the shunt `b` along with its mode, voltage band, regulated bus, and
step blocks, and a two winding transformer's magnetizing susceptance survives a
round trip through `MAG2`. The reader converts `CW` 1/2/3, `CZ` 1/2/3, and `CM`
1/2 into the neutral tap ratio, system base impedance, and magnetizing
admittance, and fresh output uses the electrically equivalent canonical
`CW = CZ = CM = 1` representation.

### UCTE-DEF

PowerIO reads UCTE-DEF revisions 2003.09.01 and 2007.05.01 and writes
2007.05.01 under the token `ucte` (alias `uct`, extension `.uct`). The column
layout is the one PowSybl Core's
[`UcteRecordParser`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ucte/ucte-network/src/main/java/com/powsybl/ucte/network/io/UcteRecordParser.java)
reads for both revisions; a 2003 file leaves the element name columns blank.
The reader maps the `##N` nodes to buses named by their 8 character node code,
with a bus id equal to the node's position in the block and the base kV taken
from the voltage level digit (750, 380, 220, 150, 120, 110, 70, 27, 330, or 500
kV). Each `##Z` country is a `ControlArea` area named by its ISO code, and the
cross border nodes (country letter `X`) form one `CrossBorder` area named `XX`,
so a tie line keeps both ends and the cross border node's own load and
generation. Node types PQ, PU, and UT map to PQ, PV, and reference; type QT (Q
and angle constant) reads as PQ with a warning. A node's load and generation
become one load and one generator, with the PowSybl consistency rules applied
and reported: a missing set point reads as zero, a missing limit as 9999,
inverted limits are swapped, a set point outside its limits moves the limit,
and a voltage regulating node with no voltage reference reads as PQ.

`##L` lines are branches on the voltage level base with the total susceptance
split evenly. Busbar couplers (status 2 and 7) are switches, an equivalent
element (status 1 and 9) keeps that mark in its extras, and a reactance under
0.05 ohm reads as 0.05 ohm, as PowSybl reads it. A coupler whose two node codes
are equal is ignored with a diagnostic, following PowSybl Core's
[`UcteImporter`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ucte/ucte-converter/src/main/java/com/powsybl/ucte/converter/UcteImporter.java#L350-L363),
and a line joining two voltage levels is refused, as PowSybl refuses it. `##T`
transformers are branches from the regulated winding (node 2) to the non
regulated winding (node 1), whose voltage level carries the impedance; the
rated voltages set the tap, and the magnetizing admittance sits on the
regulated side. `##R` phase regulation multiplies the tap by
\\(1 + n' \delta u / 100\\) and becomes a voltage control when it has a target,
while angle regulation applies the PowSybl asymmetrical or symmetrical formulas
to the tap and the phase shift and becomes a disabled active flow control. A
record with both folds the phase regulation's ratio into the angle formula, as
PowSybl does, instead of multiplying the two results. Both regulations stay in
the branch extras so that fresh UCTE output can write them back as they were
read. `##TT` special descriptions stay in the transformer extras and `##E`
exchange schedules stay in the retained source only; both are reported with
`READ.UCTE.RETAINED_SOURCE_ONLY`. UCTE uses physical quantities and has no
system MVA base, so the balanced calculation view uses 100 MVA as its internal
normalization and does not report a missing source value. Each finding points
at its record through a source span.

Fresh output writes nodes grouped by country in bus order. A bus keeps its name
when it is a UCTE node code; otherwise it receives
`<country><spot><level><busbar>`: the country letter of its area's ISO name,
else the area number's entry in the UCTE country table in ISO order; the bus id
in base 36 as the five character spot; the voltage level digit nearest its base
kV (380 kV when the bus has none); and busbar `1`, bumped on a collision. Each
derived code is reported with `EMIT.UCTE.VALUE_SUBSTITUTED`. A base kV that is
not a UCTE level is written under the nearest level with a warning; ohm, kV,
MW, and ampere values stay physical, so reading the file back expresses them
per unit on that level. A phase shift becomes a one step symmetrical angle
regulation, its step solved against the phase regulation written beside it, and
a voltage control with a tap range becomes a phase regulation; a line joining
two voltage levels is written as a transformer. UCTE requires a node's
generation bounds to contain its dispatch, so the writer widens an inconsistent
interval and reports the substituted bounds. An out of service generator
contributes no dispatch but still supplies the node's plant type letter, which
keeps that source classification stable on readback. Shunts, HVDC, storage,
static VAR compensators, three winding transformers, costs, capability columns,
angle limits, voltage bands and angles, rate B and C, remote regulation, and a
frequency other than 50 Hz are reported as dropped.

### PowerWorld

`.aux` is read and written, `.pwb` binary cases are read only, and a `.pwd`
display file parses to a `GeoLayer`
([Geographic and display data](geo-and-display.md)). `.aux` has no system base,
so the reader defaults to 100 MVA. No third party `.aux` reader exists, so the
writer is validated by PowerIO's own read back plus a PowerModels JSON bridge.
The `.pwb` layouts are reverse engineered; the decode evidence and coverage
matrix are maintainer notes at
[`powerio-tx/src/format/powerworld/FORMAT.md`](https://github.com/eigenergy/powerio/blob/main/powerio-tx/src/format/powerworld/FORMAT.md).

### PSLF

`.epc` is read and written. The reader maps the static power flow core: buses,
lines, two and three winding transformers, generators, loads, fixed shunts,
controlled shunts at their initial `g/b`, and limited two terminal DC records.
Three winding transformers are kept as typed records, and the indexed view
lowers each one as a star into \\(Y_{\mathrm{bus}}\\)/connectivity. Unsupported
sections stay in the retained source text and produce diagnostics.

### MATPOWER

Canonical MATPOWER output, for a case that did not start as MATPOWER, omits
dcline; the byte exact echo path keeps it when the case was read from MATPOWER.
Storage is written as an `mpc.storage` block.

### egret

egret output writes HVDC as `dc_branch`, the element its reader already reads,
so the power, voltage, and loss fields survive a round trip; a dcline cost
curve and storage are the only things dropped. The reader takes the power flow
ModelData subset (numeric bus ids, scalar values). A document whose
`system.time_keys` vary the scalar profile parses to
`TimeSeries<BalancedNetwork>` through `powerio::parse`, whereas the component
crate's `powerio_tx::parse` reads only the static profile and refuses it.

### pandapower JSON

The pandapower JSON writer lays the power flow core out as split oriented
`pandapowerNet` tables. Line ohms are referred to the from bus voltage, as
pandapower's `build_branch` reads them, and a bus with baseKV 0 writes `vn_kv`
set to \\(1\\) (warned) so the per unit impedances survive. A branch with a
tap, a shift, or terminals on two voltage levels becomes a `trafo` row with
`tap_changer_type = "Ratio"`; because pandapower's magnetizing model is
inductive only, its MATPOWER charging b goes out as one bus shunt per terminal
(warned, \\(Y_{\mathrm{bus}}\\) exact). The file is labeled with `f_hz` set to
\\(50\\) and `c_nf_per_km` compensated, so a 60 Hz source keeps its exact
\\(Y_{\mathrm{bus}}\\). A reference bus without a generator gets an `ext_grid`
row, which reads back as a Ref generator. The writer also warns on dropped
HVDC, storage, capability columns, angle limits, rate B/C, nonfinite values
(written as JSON null), and costs `poly_cost` cannot carry. The reader models
ratio, ideal, and pandapower 2.x tap changers, off nominal
`vn_hv_kv`/`vn_lv_kv`, lv side taps, and shunt `vn_kv` scaling; ZIP load
composition, line shunt conductance, magnetizing branches, tabular tap
changers, reactive cost coefficients, and any other table with rows warn with
row counts.

### PyPSA CSV folders

PyPSA CSV folders are canonicalized directory outputs rather than byte exact
text conversions. The mapping covers static buses, generators, loads, lines
(ohms on the bus0 voltage, as PyPSA computes them), transformers (rebased
between the system base and the transformer `s_nom`), shunts, storage units,
and base MVA. The reader maps links to HVDC with a warning, requires `v_nom`
and balanced CSV quoting, and warns on stores, nonzero `g`, and each CSV it
does not read (time series, carriers). The writer keys tables by bus name,
falling back to the numeric id when names collide (warned), and warns on
dropped HVDC, q limits, mbase, transformer angle limits, rate B/C, isolated
buses, nonfinite p limits, and slackless or normalized networks. Nonnumeric bus
names read back as dense synthetic ids with the originals on `Bus.name`.

### DOE GO Challenge 3 JSON

DOE GO Challenge 3 JSON is a grid exchange format “for Challenge 3 and beyond,”
so the format name is broader than any one calculation type. PowerIO recognizes
its Challenge 3 input/problem data file and returns `AcScucInstance`. The
instance has the declared time points and durations, initial commitment and
dispatch, time varying bounds, costs and reserves, energy windows,
contingencies, and one shared `BalancedNetwork`, which `instance.network()`
returns. One directory or memory `Source` containing both the input/problem
data file and its matching output/solution data file returns `AcScucSolution`;
an output/solution data file on its own is rejected, because it has neither the
component definitions nor the time axis. Problem data is parse only. A complete
`AcScucSolution` emits the official output/solution data file, including bus
voltage, shunt step, device commitment, dispatch and reserves, AC line status,
transformer tap, phase shift and status, and DC line terminal power fields. The
pinned GO-3 model validates PowerIO's small problem and output documents and
all of the D1/D2/D3 input/problem data files, and C3DataUtilities reports no
data, ignored, or solution errors for those documents. The older pinned GO-3
model and D1/D2/D3 files still have `network.bus.con_loss_factor`, a field that
version 1.1.1 of the data format removed; PowerIO keeps the original source and
reports one bounded diagnostic instead of treating it as an electrical network
or AC SCUC field. Optional bus location labels and incomplete coordinate pairs
stay in `Bus.extras`, and optional consumer descriptions, voltage setpoints,
and nameplate capacities stay in `Load.extras`; each reports
`READ.GOC3.OPTIONAL_FIELD_UNTYPED`. A producer description has no generator
metadata field to go to and reports `READ.GOC3.RETAINED_SOURCE_ONLY`.

### Surge JSON

PowerIO reads and writes the versioned `surge-json` network document. The
reader maps buses, loads, fixed shunts, branches, generators, storage, and HVDC
links into `BalancedNetwork`, keeps the original source for same format echo,
and warns about source sections that stay only in the retained document. The
writer emits a canonical Surge network body for the supported power flow core,
and richer MATPOWER generator capability or ramp columns and unsupported cost
shapes are reported in the emission diagnostics. An HVDC link has the terminal
voltage setpoints, the reactive limits, and the loss model on its converter
terminals; a Surge link has no terminal reactive flow, no cost curve, and no
received power (the reader derives it from the setpoint and the loss model), so
those are warned. A link with converter or control detail beyond the neutral
converter this writer emits (firing angles, converter transformer taps,
commutation impedance, a DC voltage schedule) is warned on the way in.

### DeepMind OPFData JSON

The DeepMind OPFData reader takes one raw JSON document from a FullTop or N-1
release into the balanced transmission model. Topology, limits, loads, shunts,
and quadratic costs come from `grid`; solved bus voltages, generator dispatch,
and branch flows come from `solution`. Powers and ratings are converted from
per unit, angles from radians, link indices from zero based to one based, and
flow columns from `[pt, qt, pf, qf]` into the canonical terminal order.
Original bus IDs and names, areas and zones, and frequency are absent, and the
solver's initial generator values differ from the solved snapshot, so a
conversion to another format reports those facts. The adapter works from the
feature widths and the row and link counts in each file rather than from a case
name registry or expected element counts, so the same path covers all published
grid families (14 through 13,659 buses) and both FullTop and N-1 examples;
generator and branch outages appear as absent rows and links and are validated
against that example's solution topology. The published releases are derived
from PGLib-OPF cases, but the reader does not use PGLib case names or a case
registry, so a document from another source is accepted when it follows the
same object layout, feature column order, units, and link rules. The paper's
Appendix A is the published format definition and the PyTorch Geometric loader
is the executable reference. No separate JSON Schema or format version marker
is published, so a document that departs from that layout is rejected by the
reader's shape and topology checks. An unrecognized object field stays in the
retained source and produces a projection warning, so same format echo still
works. The raw source echoes byte exactly; there is no canonical writer, `.pt`
cache reader, archive reader, downloader, or batch directory API.

### IEEE Common Data Format

IEEE Common Data Format (`ieee-cdf`, alias `cdf`) is read only. The reader
takes the title card MVA base and date; the bus records (number, name, area,
loss zone, type, solved voltage and angle, load, generation, base kV, desired
voltage, MVAr or voltage limits, shunt G and B, remote controlled bus); the
branch records (tap and Z bus, circuit, type, R, X, B, the three MVA ratings,
control bus and side, turns ratio, phase shift angle, tap limits, step size,
and the controlled quantity limits); and the interchange records (area number,
slack bus, export, tolerance, code, and name). The tap bus is the from bus and
a nonzero turns ratio marks a transformer. A blank branch type reads as a
transmission line, as the PowSybl reader reads it; a type 1 through 4 branch
without a ratio reads as unity; and types 2, 3, and 4 get a regulating
transformer control block whose `ntp` derives from the step size. A type 1 bus
reads as PQ with a fixed reactive generator and the limit columns as its
voltage band, and every type 2 or 3 bus, along with any bus with nonzero
generation, gets a generator. The format has no active power limits, no machine
base, and no voltage limits, so `pmin` 0 MW, `pmax` 9999 MW, `mbase` equal to
the system base, and `vmax`/`vmin` 1.1/0.9 p.u. are assumed and reported as
`READ.IEEE_CDF.VALUE_DEFAULTED`. Loss zone names, tie lines, the branch area
and loss zone columns, alternate swing bus names, and the title originator,
year, and season survive in the retained source only
(`READ.IEEE_CDF.RETAINED_SOURCE_ONLY`).

A record cut before a mandatory field reads that field as zero and reports
`READ.IEEE_CDF.RECORD_TRUNCATED` with the record's span. A header item count,
terminator, misplaced record, zero impedance branch, or undeclared bus
reference is `READ.IEEE_CDF.SOURCE_MALFORMED`, and a type or side code outside
the documented set is `READ.IEEE_CDF.VALUE_SUBSTITUTED`. A title card without a
positive MVA base, or a record whose bus numbers or numeric fields cannot be
decoded, ends the read with a spanned `PARSE.IEEE_CDF.MALFORMED`. The column
ranges follow PowSybl Core's
[`IeeeCdfBusReader`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ieee-cdf/ieee-cdf-model/src/main/java/com/powsybl/ieeecdf/model/reader/IeeeCdfBusReader.java#L26-L51)
and
[`IeeeCdfBranchReader`](https://github.com/powsybl/powsybl-core/blob/0939bfcc2c0c094de907dc818dd688b4cbfb7281/ieee-cdf/ieee-cdf-model/src/main/java/com/powsybl/ieeecdf/model/reader/IeeeCdfBranchReader.java#L27-L62),
which read the public archive files; those files place the last two branch
limits one column to the left of the 1973 table, and the reader accepts both
layouts. A `.txt` or `.cdf` file whose first card is a CDF title card is
detected without a declared format. Fresh output of this format is not
required, so there is no writer; `emit` to `ieee-cdf` is refused as read only,
and a case converts to any writable format instead. The PowSybl gate reads
every public IEEE case with PyPowSybl's own CDF importer, compares its bus,
branch, generator, load, and shunt counts and its load and generation totals
with the PowerIO parse, then reloads fresh MATPOWER written from each case.

### GridFM Parquet datasets

GridFM Parquet datasets (behind the `gridfm` feature, following the
[GridFM data kit output schema](https://gridfm.github.io/gridfm-datakit/manual/outputs/))
parse to a scenario set of balanced networks over one shared element identity
map. Each scenario recovers the complete native balanced table data: bus types,
voltages, and limits; nodal load and shunt totals; generator dispatch, bounds,
and the `cp0`/`cp1`/`cp2` polynomial as given; branch
`r/x/b/tap/shift/rate_a`/angle limits and `pf/qf/pt/qt` terminal flows; and
`baseMVA`. Dense bus indices, nodal demand records, and line classification for
a unit tap with zero shift are GridFM source facts and do not produce reader
diagnostics.

Writing a richer network reports the projections into GridFM's fixed tables:
source bus renumbering, several loads or shunts combined at one bus, metadata
and equipment with no column, and costs outside the fixed quadratic
representation. If a branch has no solution in the source, the writer evaluates
`pf/qf/pt/qt` from the stored bus voltages and reports that derived value.
Generated component identities give PowerIO stable references and are not
treated as source metadata. A native GridFM network writes without findings.
[Matrices and graphs](matrices.md) describes the dataset the writer produces.
Both directions need the `gridfm` cargo feature, which the CLI and the Python
wheel include.

### IIDM versions

PowerIO reads every IIDM serialization version PowSybl has published and writes
1.17. A document of an older version is read with that version's rules and
reported with `READ.XIIDM.VERSION_COMPATIBILITY`, and a namespace naming a
version outside this table is refused with `PARSE.XIIDM.VERSION_UNSUPPORTED`.
The table lists, per version, what the reader does differently from 1.17, and
each row is checked against the PowSybl fixture of the same network in
`tests/data/xiidm/powsybl`.

| Version | Read | What the version states differently |
| --- | --- | --- |
| 1.0 | XIIDM | iTesla namespace. Busbar sections carry the calculated bus `v` and `angle`. Three winding transformers have no `ratedU0` (leg 1 rated voltage is the impedance base), no leg 1 tap changer, and no phase tap changers. Tie lines state both half lines inline with `_1`/`_2` suffixes and `ucteXnodeCode`. Shunts state `bPerSection`, `maximumSectionCount`, and `currentSectionCount` and never regulate. Static VAR compensators spell `voltageSetPoint` and `reactivePowerSetPoint`. Batteries spell `p0` and `q0`. Ratio tap changers state `targetV`. Loading limits sit directly on the equipment as the selected `DEFAULT` group. A switch closing a bus or node onto itself is discarded, as PowSybl does. No `minimumValidationLevel`. |
| 1.1 | XIIDM | PowSybl namespace. Calculated buses under node breaker topology. Three winding transformer `ratedU0`, leg 1 tap changers, and phase tap changers. |
| 1.2 | XIIDM | `fictitious` on every identifiable, `targetDeadband` on tap changers, `ratedS`, and shunt voltage regulation. |
| 1.3 | XIIDM | Shunt linear and nonlinear models with `sectionCount`, aliases, and boundary line generation. |
| 1.4 | XIIDM | Alias types. |
| 1.5 | XIIDM | Active and apparent power limits. |
| 1.6 | XIIDM | Voltage levels outside substations and VSC regulating terminals. |
| 1.7 | XIIDM | `minimumValidationLevel` and the equipment validation namespace. |
| 1.8 | XIIDM | Batteries spell `targetP` and `targetQ`. Fictitious bus injections (reported, not retained). Self connected switches are refused. |
| 1.9 | XIIDM | Shunt `p`. |
| 1.10 | XIIDM | Tie lines reference two dangling lines. Load models. |
| 1.11 | XIIDM, JIIDM | `pairingKey` replaces `ucteXnodeCode`. Subnetworks. Voltage angle limits (reported, not retained). |
| 1.12 | XIIDM, JIIDM | Operational limits groups and ratio tap changer `regulationMode`/`regulationValue`. |
| 1.13 | XIIDM, JIIDM | Areas, `isCondenser`, and active power control 1.2. |
| 1.14 | XIIDM, JIIDM | Solved tap positions and section counts, `regulating` on static VAR compensators and phase tap changers. |
| 1.15 | XIIDM, JIIDM | DC nodes, grounds, lines, switches, and AC/DC converters. |
| 1.16 | XIIDM, JIIDM | `shuntCompensator` and `boundaryLine` element names and multiple selected limit groups. |
| 1.17 | XIIDM, JIIDM | Optional zero conductance and susceptance, `retained` and `lowTapPosition` defaults, DC switch resistance. Writers use this version. |

JIIDM has no namespace, so the document's `version` field selects the same
rules and `minimumValidationLevel` selects the validation level. PowSybl ships
JIIDM fixtures from 1.11 on; an older `version` value reads with that version's
XML rules.

## Missing generator costs

PSS/E `.raw` files have no generator cost curves. Converting a PSS/E case to
MATPOWER writes `mpc.gen` and omits `mpc.gencost` with a warning, because
powerio does not invent zero costs. If your workflow needs costs, pick a policy
explicitly:

```sh
powerio convert case.raw --from psse --to matpower --missing-gen-cost zero -o case.m
powerio dcopf case.m -o out --missing-gen-cost quadratic --default-gen-cost 0.01,2.0,0.0
powerio gridfm case.raw --from psse -o out --missing-gen-cost zero
```

- `preserve`: leave missing costs absent (default for conversion and GridFM export);
- `require`: fail on an in service generator without cost (default for DC OPF export);
- `zero`: fill missing rows with a MATPOWER polynomial cost `[0, 0, 0]`;
- `quadratic`: fill missing rows with `--default-gen-cost C2,C1,C0`.

`--gen-cost-csv` overrides costs by generator row before the missing cost
policy runs. The header is `gen_index,bus,c2,c1,c0,startup,shutdown`:
`gen_index` is zero based in the current generator table, `bus` must match that
generator's bus id (which catches a stale table after reordering), and
`startup`/`shutdown` default to zero. GridFM stores `cp0/cp1/cp2` columns;
missing or unsupported costs still write zero columns, and the manifest
separates `missing_cost_gens`, `unsupported_cost_gens`, `zeroed_cost_gens`, and
`synthesized_gen_costs`.
