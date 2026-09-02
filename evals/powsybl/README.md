# PowSybl interoperability check

The [CGMES contribution audit](cgmes-contribution-audit.md) compares the final
implementation with Mohamed Numair's original CGMES reader and writer commits.

The gate keeps the case9 XIIDM, JIIDM, CGMES, PSS/E RAW revisions 33 through
35, and PSS/E RAWX smoke outputs. It also reads official PowSybl Core reference
cases for CGMES 2.4.15, CGMES 3.0, XIIDM 1.12 through 1.17, RAW, and RAWX, and
it asks PowSybl to write the eurostag tutorial network as XIIDM and JIIDM in
every IIDM version from 1.0 to 1.17; PowerIO reads each of those, emits the
same encoding at 1.17, and PyPowSybl compares both against the official
network. CI obtains
those files with a sparse checkout of `powsybl/powsybl-core` at
`0939bfcc2c0c094de907dc818dd688b4cbfb7281`; no PowSybl case is copied into
this repository.

PowSybl Core 7.3.0 reads RAW revisions 32, 33, and 35, but not revision 34.
The gate loads fresh PowerIO RAW 33, RAW 35, and RAWX 35 output with PowSybl
and records PowSybl's expected rejection of fresh RAW 34. PowerIO's parser,
writer, and round trip tests cover revision 34 directly. PowerIO reads RAW 32
and writes it fresh at revision 33; the gate reads two official RAW 32 cases
and loads the fresh RAW 33 written from each.

The gate also asks PowSybl to export the same remote regulation case as XIIDM
1.12 through 1.17, CGMES 2.4.15 on CIM16, and CGMES 3.0 on CIM100. PowerIO
reads each PowSybl output, serializes and deserializes it through PowerIO IR,
emits XIIDM 1.17 or CGMES 3.0 from the typed value, and compares both PowSybl
views. This
checks the supported input revisions and profiles against files written by
PowSybl rather than only against files stored in its source tree. The remote
regulation case is also emitted as JIIDM, which PyPowSybl loads through
its JIIDM importer and compares against the official network.

PowSybl writes human readable, non-UUID mRIDs in these two CGMES cases.
PowerIO writes UUID mRIDs as required by its fresh CGMES output rules. The gate
does not mistake that deliberate change for lost equipment. It reads the
PowerIO IR identity metadata, requires the exact
`EMIT.CGMES.VALUE_SUBSTITUTED` line for all 172 CIM16 identifiers and all 183
CIM100 identifiers, and checks the UUIDv5 result for every retained typed
identity. Equipment is paired by its exact retained name and electrical role.
The raw XML checks map TopologicalNode, Terminal, and ConnectivityNode mRIDs.
Tap and operational limit records are paired by transformer or equipment,
terminal side, position, limit kind, duration, and limit group. The gate prints
SHA-256 digests of both complete identity inventories so an unreviewed mapping
change cannot disappear in abbreviated output.

PowSybl node numbers are local IIDM indices, not CGMES identities. A fresh
reload can assign different numbers while retaining the same ConnectivityNode,
TopologicalNode, terminal, and calculated bus. The generated CGMES comparison
therefore checks those CGMES identities and relationships instead of comparing
the local node integers. XIIDM and PSS/E checks still compare node numbers
exactly because those formats carry them directly.

Every official input passes through two commands before inspection:

```text
source -> powerio serialize -> PowerIO IR -> powerio convert -> fresh output
```

The IR step removes retained source bytes. A same format writer therefore has
to rebuild the file from the parsed power network.

PyPowSybl loads each fresh output and IIDM validation must reach
`STEADY_STATE_HYPOTHESIS`. The focused checks cover CGMES mRIDs, the
SeriesCompensator class and its zero sequence and varistor properties, remote
generator voltage regulation, the standard SynchronousMachine reactive
capability curve, the generator service status carried from its official CGMES
3.0 SvStatus into fresh SSH and SV profiles, CGMES 2.4.15 VSC HVDC data, XIIDM
terminals with absent active and reactive power, switched-shunt sections, node
breaker generator regulation, three winding transformer taps, equipment names,
and RAWX null substation node voltages.

The merged MicroGrid folders contain profiles from several modeling
authorities. PowerIO projects boundary and tie equipment into its balanced
network before fresh CGMES emission. The resulting ordinary line, load, and
bus counts differ from PowSybl's source view by design. The gate checks the
audited source counts, confirms that fresh output contains no PowSybl boundary
or tie line objects, and compares the mRIDs of equipment shared by both views.
It does not require all source and fresh counts to match. The CGMES 2.4.15
input uses CIM16; PowerIO's `cgmes` target writes CGMES 3.0 on CIM100, so the
gate checks both namespaces and the electrical values carried across that
projection.

PowSybl's mapped equipment dataframes must retain every source name. The XML
check compares every source equipment mRID that carries an
`Equipment.EquipmentContainer` reference because IIDM does not expose that CIM
relationship uniformly. CGMES 3.0 must retain 57 of 57 such names and
container relationships. CGMES 2.4.15 must retain all 82 names and container
relationships, including its one-terminal Junction and all 13 BusbarSections.
Of the 82 raw container mRIDs, 81 remain byte-for-byte equal. Junction
`5249A78F-6642-4fc5-968F-06E2ED18FAB7` keeps name `Junction_XJ1` and its
relationship to Line `TieLine_XWI_GY11`; the source Line uses non-UUID RDF ID
`f03d65b2a51049ffa533e433721145c1_X`, so fresh CIM100 uses deterministic UUID
`6cd9f4f5-8185-57c9-a8a3-53de5e0ddeb1`. Boundary and tie equipment still
follows the multi authority projection above rather than a same-class PowSybl
row comparison.

For equipment mRIDs shared by the source and fresh PowSybl views, the gate also
uses an explicit electrical column list and a relative tolerance of `1e-9` and
absolute tolerance of `1e-8` for numbers. It compares:

- ordinary line `r`, `x`, end `g` and `b`, voltage levels, and both terminal
  connection flags;
- two winding transformer `r`, `x`, `g`, `b`, rated voltages, effective ratio
  `rho`, phase `alpha`, voltage levels, and both connection flags;
- three winding transformer `r`, `x`, `g`, `b`, rated voltages, effective
  `rho`, phase `alpha`, voltage levels, and all three connection flags;
- generator energy source, active and reactive targets and bounds, rated
  apparent power, voltage target, reactive limit kind, voltage control flag,
  exact regulated element, voltage level, and connection flag;
- load scheduled active and reactive power, voltage level, and connection
  flag;
- shunt conductance, susceptance, selected and maximum section counts, voltage
  target and deadband, model kind, voltage control flag, voltage level, and
  connection flag; and
- static var compensator susceptance bounds, voltage and reactive targets,
  control mode and flag, exact regulated element, voltage level, and connection
  flag;
- VSC converter loss factor, reactive bounds, voltage and reactive targets,
  reactive limit kind, voltage control flag, exact regulated element, voltage
  level, connection flag, and HVDC line assignment; and
- HVDC line active power target and bound, nominal voltage, resistance,
  converter mode and station assignments, and both connection flags.

Switch checks compare every physical switch mRID, name, CIM class, open and
normal open position, retained flag, voltage level, and fictitious flag. PowSybl
creates extra fictitious switches for disconnected terminals; their IDs and
local node numbers are import products, so the gate compares their electrical
role and count while the raw terminal checks cover the underlying connectivity.

The operational limit comparison uses the complete row identity: equipment
mRID, side, limit kind, acceptable duration, and limit group mRID. It then
compares the limit value, equipment kind, fictitious flag, and selected flag.
CGMES 3.0 has 25 source rows and no additional fresh row. CGMES 2.4.15 has 93
source rows, all retained, plus 31 fresh zero-second current rows at the
projected 90 percent rating. CIM16 limit display labels such as `CL-1` gain an
equipment name prefix in the CIM100 view, so the display label is not compared;
the limit group mRID remains part of the exact row identity.

The raw XML check also pins every `OperationalLimitType` kind. The CGMES 3.0
source contains two PATL and two TATL records; fresh output contains one of
each. The CGMES 2.4.15 source contains three PATL, three PATLT, six TATL, and
three TCT records; fresh CGMES 3.0 contains one PATL and three TATL records.
PowerIO reports each PATLT or TCT reduction to the permanent or temporary
limit model instead of presenting the fresh kind as exact source retention.
Each PowSybl generated CGMES 2.4.15 and 3.0 reference contains one PATL type;
the fresh CGMES 3.0 output must contain the same one PATL type.

PowSybl exposes only a collapsed subset of boundary limits, so the boundary
comparison reads the raw official XML, the retained PowerIO IR limit groups,
and fresh XML. CGMES 3.0 must retain 20 `OperationalLimitSet` groups with 40
CurrentLimit rows. CGMES 2.4.15 must retain 24 groups with 162 permanent and
temporary CurrentLimit rows. The comparison pins equipment, terminal side,
limit name and value, duration, and fictitious flag.

Tap changer rows are keyed by transformer mRID and winding side. The main row
comparison covers selected position, low and high positions, step count, load
tap capability, regulation flag, target and deadband, control mode for phase
changers, and regulated side. Step rows add the exact position to that key and
compare ratio `rho`, phase `alpha` where exposed, and step `r`, `x`, `g`, and
`b`. CGMES 3.0 has 3 ratio changers with 97 steps and 4 phase changers with 125
steps. CGMES 2.4.15 has 10 ratio changers with 271 steps and 1 phase changer
with 25 steps.

Calculated bus IDs are matched through shared terminal signatures because the
multi authority topology projection changes their derived spelling. Boundary
terminal topology is checked directly in raw XML: equipment and side,
TopologicalNode, and retained or projected ConnectivityNode. The gate compares
voltage level identity and terminal connection flags on the PowSybl rows.
SV quantities use a separate strict comparison with the same numeric tolerance
and NaN handling. Buses are matched by the exact set of shared equipment
terminals rather than their calculated bus IDs. The gate compares bus voltage
magnitude and angle; line and transformer terminal active and reactive power;
and generator, load, shunt, static var compensator, and VSC converter active
and reactive power. CGMES 3.0 maps every source calculation bus. CGMES 2.4.15
maps every source calculation bus except
`bccf01d1-680c-4683-91b9-9d19748519cb_0` and
`d966db16-ec73-4c00-a9bc-8aae3aaa0573_0`; every fresh non-boundary calculation
bus must still match.

Those two buses are the exact observable effect of a PowSybl 7.3.0 limitation,
not a general topology exception. The PowerIO IR retains side TWO connected on
transformers `d1494778-e194-4ee5-84ec-ac8024375e4f` and
`b59c4282-a1f7-4ed2-a6de-2a5b97c03f64`. Their configured buses are
`5cae357a-731f-4ab1-b133-4f90795206c7` and
`f3c8e35e-744f-431a-8505-dfe3c275616a`, shared only with VSC converters
`d74faa88-d996-4de3-be52-f5db28dd3fb8` and
`dde55045-9d3e-43b8-8d28-0585e30cd6b1`. Each DC island contains a
`DCSeriesDevice`; PowSybl reports the projected island as unsupported
`BACK_TO_BACK`, drops its converter, and reloads the transformer terminal
disconnected. The gate requires the two exact `EMIT.CGMES.RECORD_DROPPED`
messages and verifies that the IR connection remains true.

CGMES 2.4.15 boundary `SvPowerFlow` records remain present in fresh XML on the
12 projected EquivalentInjection load terminals. The checker compares their
raw active and reactive values instead of treating PowSybl's load dataframe
`p` and `q` as independent solution fields. For CGMES 3.0, ten
cross-authority `SvPowerFlow` rows, or 20 active and reactive quantities, cannot
be assigned to equipment from the other authority. Fresh line terminal values
must remain absent and each exact source mRID, terminal, equipment, value, and
`READ.CGMES.RECORD_UNMAPPED` diagnostic is required.

Five CGMES 3.0 boundary TopologicalNodes also carry distinct voltage
observations from paired authorities that one projected calculation bus cannot
represent. The exact source SvVoltage mRID, TopologicalNode, magnitude in kV,
and angle in degrees are:

```text
cf29c76d-daaf-4f4c-b4c7-8a8c46089fbd  1a4d0f02-6adb-4fa2-a828-b0366497ae5a  414.0277   339.5112
9408a842-1d22-49ef-abd2-d3cfa8b087b8  eff86931-991c-4f6f-b5d6-75dad40e0a12  412.6039   339.568
721de4d4-4271-4ef6-8f8e-22b29bc213c3  ebd2d2fd-3cfc-466d-a542-104ce0e8fd4d  223.2214    -9.052585
3c68ee89-7a4f-4aeb-8c95-9356398d204c  994d58d1-48c9-477d-8ba4-ec143e714ba1  412.8738   339.5562
bf4d0111-51f6-4c69-934e-8ffaefe6ff15  88f1e099-9930-4ad2-a477-f301e35ca9a4  223.2396   -11.03524
```

The gate requires one structured diagnostic for each record, exact authority
IDs for Elia and TenneT, and absence from fresh SV. Terminal current is derived
from voltage and complex power and is not an independent SV input.

Transformer `rated_s` is not an independent field in the PowerIO transformer
model; exact current limits cover loading capability. Solved tap positions and
tap regulating bus IDs are SV or topology projections; selected positions,
regulated sides, targets, and complete step tables are compared instead.
Generator reactive bounds evaluated at the terminal `p`, transformer impedances
evaluated at the current tap, shunt solved section counts, and terminal voltage
and angle on equipment rows are derived columns rather than independent inputs.
The standard generator capability curve points and CGMES 3.0 service flag are
checked separately against their official mRIDs. The gate also compares all 52
official CGMES 3.0 SvStatus equipment references and service flags with the
fresh SV profile. The CGMES 2.4.15 Type 2 set has an SV profile with power flow
and voltage observations but no SvStatus records, so it has no source service
flags to compare.

Three node breaker sets from the pinned checkout check CGMES import without
a TP profile: the CGMES 2.4.15 MiniGrid and SmallGrid node breaker base
cases with their EQ_BD and TP_BD documents under
`conformity/cas-1.1.3-data-4.0.3`, and the CGMES 3.0 MicroGrid under
`cgmes3-test-models`. The gate stages each set twice, as shipped and without
its TP and TP_BD documents, reads both with PowerIO, and emits fresh CGMES
from the calculated topology. It requires exactly one
`READ.CGMES.TOPOLOGY_CALCULATED` remark with the pinned bus, ConnectivityNode,
closed switch, and open switch counts (13 buses from 103 nodes and 90 closed
switches; 118 from 1225 and 1107; 16 from 42 with 25 closed and 1 open), an
empty `bus_breaker_buses` table, one `calculated_buses` record per bus, a
bus `uid` equal to the UUIDv5 under the PowerIO CGMES namespace of
`calculated-bus:` followed by the sorted ConnectivityNode mRIDs of that
record, and no bus identity equal to a ConnectivityNode mRID. It then keys
every connected terminal of PowSybl's bus view by its CGMES terminal alias
and requires the grouping of those terminals into buses to equal PowerIO's
grouping through `terminals[].node` and `connectivity_nodes[].calculated_bus`:
47 shared terminals in 11 buses, 617 in 115, and 49 in 11. The MiniGrid and
SmallGrid calculated groupings also equal the ConnectivityNode grouping their
shipped TP states. The CGMES 3.0 MicroGrid TP, dated later than its SSH,
separates the ends of breaker `5f5d40ae-d52d-4631-9285-b3ceefff784c` that the
SSH closes; PowSybl follows the SSH position and so does the calculated
topology, so that grouping is pinned as differing from the shipped TP (18
buses against 16). Finally PowSybl loads the fresh CGMES emitted from each
calculated topology, must reach `STEADY_STATE_HYPOTHESIS`, and must group the
shared terminals as the official set does (38, 557, and 49 shared terminals;
12, 118, and 16 fresh bus view buses). PowSybl's bus view omits a component
with no busbar section and fewer than two feeders, so its bus counts stay
below PowerIO's.

The two XIIDM inputs use exact source and fresh identity sets. The gate compares
voltage levels, calculated buses, lines, two winding transformers, generators,
loads, shunts, operational limits, and ratio tap changer rows and steps. The
HVDC case also compares both LCC converters and the HVDC line. These checks
cover names, equipment parameters, voltage level and bus assignments,
connection flags, controls, schedules, and solved voltage and active and
reactive power. The remote control case performs 748 listed field comparisons;
the HVDC case performs 667. Absent LCC terminal active and reactive power must
remain absent in both the dataframe and fresh XIIDM XML.

The gate also reads PowSybl's
`threeWindingsTransformerToBeEstimated.xiidm` reference file at each supported
input version, XIIDM 1.12 through 1.17. Each source passes through PowerIO IR
and fresh XIIDM 1.17 output before PowSybl reloads it. The comparison covers
the three voltage levels and calculated buses, the generator and two loads,
the three winding transformer, both ratio tap changers, and all six tap steps.
The script pins the SHA-256 of every reference file and requires the exact
diagnostic for the PowSybl extension that is outside PowerIO's electrical
model. These files remain in the pinned PowSybl Core checkout under its
MPL-2.0 license; none is copied into PowerIO.

The pinned PowSybl `five_bus_nodeBreaker_rev35.xiidm` case supplies a nonempty
XIIDM switch comparison. PowerIO reads and freshly emits all 21 node breaker
switches, and PowSybl reloads the result. The check compares each switch ID,
name, kind, open position, retained flag, voltage level, node endpoints, and
fictitious flag. The fixture stays in the PowSybl checkout under MPL-2.0 and
is pinned by SHA-256.

The RAW and RAWX inputs use the same source and fresh dataframe comparison for
voltage levels, calculated buses, lines, two and three winding transformers,
generators, loads, shunts, operational limits, and ratio tap changer rows and
steps. It covers names, electrical parameters, terminal active and reactive
power, voltage level and bus assignments, node numbers, connection flags,
controls, and solved values. PSS/E fixed width identifiers and text fields are
compared after removing trailing ASCII space padding; no other identity or text
normalization is allowed. One cell class is excluded rather than normalized:
RAW 32 and 33 branch records carry no NAME field, and PowSybl names such a
line `<bus I name>_<bus J name>_<ckt>` at import, while it reads a RAW 35 NAME
field as written. For the RAW 33 switched-shunt source the gate requires each
source line name to equal that synthesis exactly and each fresh RAW 35 NAME
field to be blank, then excludes only those line name cells; a dropped name
still fails. The switched-shunt RAW case performs 589 listed field
comparisons, the two-substation RAWX case performs 323, and the node breaker RAW
case performs 337. The existing focused checks additionally require switched
shunt section counts, the named RAWX line and transformers, explicit null RAWX
substation node voltages, and exact node breaker generator and three winding
tap regulation.

The two official RAW 32 cases, `ExampleVersion32_exported.raw` and
`IEEE_30_bus.raw` from `psse/psse-converter/src/test/resources`, pass through
PowerIO IR and fresh RAW 33 output before PowSybl reloads them. The checker
pins each file's SHA-256 and requires the source header to state revision 32
and the fresh header revision 33. The comparison is the same source and fresh
dataframe comparison as the other RAW cases, with one verified identity
exception.
PSS/E ID fields are two character blank padded strings, and PowSybl names a
fixed shunt `B<bus>-SH<ID>` with the field as written, so the `' 1'` fixed
shunts in these cases are `B7-SH 1`, `B10-SH 1`, and `B24-SH 1` in the source
view; PowerIO stores the trimmed identifier and writes `'1'`, which PowSybl
names `B7-SH1`. A shunt id is renamed for the comparison only when the source
field carries leading blanks, the fresh RAW record states the trimmed text,
and both views contain the id, so a dropped shunt still fails. Fresh RAW 33
carries no branch NAME field either, so PowSybl synthesizes the same line
names from both files and no line name cell is excluded. The RAW 32 example
case performs 403 listed field comparisons and the IEEE 30 bus case 1860.

The Python environment pins PyPowSybl 1.16.1, pandas 3.0.5, numpy 2.5.2,
networkx 3.6.1, and prettytable 3.18.0. PyPowSybl's PowSybl Dependencies
2026.1.0 release pins PowSybl Core 7.3.0. The checker asserts every runtime
version, the Core version, and the exact Core commit rather than printing them
for information.

Run it from the repository root after installing the requirements:

```sh
python3 -m venv .venv-powsybl
.venv-powsybl/bin/pip install -r evals/powsybl/requirements.txt
cargo build -p powerio-cli
bash evals/powsybl/run.sh target/debug/powerio \
    .venv-powsybl/bin/python /path/to/powsybl-core
```

## External RTE 7k check

`check_rte7000.py` runs the 33,054,521 byte RTE 7k XIIDM case through fresh
PowerIO IR and XIIDM emission, then compares every PyPowSybl table field. It
does not download or copy the dataset into this repository. The checker pins
SHA-256
`cd1cbd8c49c367ca366dd83bb05ead72f984a35de21e135e01f86b74d810a244`,
351,970 lines, the published CDLA-Permissive-2.0 license, and the audited
source row counts. Obtain the file separately from
[OpenSynth/D-GITT-RTE7000-2021](https://huggingface.co/datasets/OpenSynth/D-GITT-RTE7000-2021),
then run:

```sh
.venv-powsybl/bin/python evals/powsybl/check_rte7000.py \
    /path/to/recollement-auto-20210103-0000-enrichi.xiidm \
    --powerio target/debug/powerio \
    --output-dir /tmp/powerio-rte7000
```

The report keeps the source validation result separate from PowerIO. The
published source currently fails PowSybl validation at load `HASTI3CD1`
because its `p0` is absent. Fresh output must reproduce the source validation
result; preserving that absence must not be reported as a PowerIO defect. The
output directory contains the IR, fresh XIIDM, diagnostic logs, wall times,
source hash/license/count information, assertion totals, and the complete field
comparison. This large check is optional and is not part of CI.

Hugging Face stores the case bzip2 compressed under its day directory:
`2021/01/03/recollement-auto-20210103-0000-enrichi.xiidm.bz2` with an MD5 file
beside it. Decompress it before running the check.

### Recorded run

| Item | Value |
|---|---|
| PowerIO commit | `05de92a00dae7fa2030ec14852547d9780e929f5` |
| PowerIO binary | `target/debug/powerio` |
| PyPowSybl | 1.16.1 on PowSybl Core 7.3.0 at `0939bfcc2c0c094de907dc818dd688b4cbfb7281` |
| Source | SHA-256 `cd1cbd8c49c367ca366dd83bb05ead72f984a35de21e135e01f86b74d810a244`, 33,054,521 bytes, 351,970 lines, CDLA-Permissive-2.0 |
| Source counts | all 29 pinned table counts matched |
| Field comparison | 29 tables, 445,332 row identities, 2,514,145 field values, no differences |
| Validation | source and fresh both stop at load `HASTI3CD1` (`p0 is invalid`); the results match |
| Diagnostics | `READ.XIIDM.VALUE_DEFAULTED` 10, `READ.XIIDM.VERSION.COMPATIBILITY` 3, `READ.XIIDM.CALCULATION_VIEW` 2; identical in the stored IR and the emission log |
| Failures | none |
| Wall time | parse and serialize 231.1 s, deserialize and emit 21.7 s, PyPowSybl load 1.1 s source and 1.1 s fresh, total 255.0 s (debug build) |
