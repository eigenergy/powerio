# PowerWorld formats in powerio

Working notes for the PowerWorld interchange path: what a real complete case
export contains, where the original reader fell short, and the evidence behind
the `.pwb` binary decoding. Fixture provenance is in
`tests/data/powerworld/README.md`.

## The aux grammar (from the official guide)

Source: "Auxiliary File Format for Simulator 24", PowerWorld Corporation,
November 6, 2025 (powerworld.com). The grammar below is what the guide
specifies; Simulator 19+ writes either form.

Legacy header:

```text
DATA DataName(object_type, [list_of_fields], file_type_specifier, create_if_not_found)
{
value_list_1
...
}
```

- `DataName` is optional.
- `file_type_specifier`: blank, `AUXDEF`, or `DEF` mean space delimited
  values; `AUXCSV`, `CSV`, or `CSVAUX` mean comma delimited.
- `create_if_not_found`: `YES`, `NO`, or `PROMPT`; optional, default `YES`.

Concise header (Simulator 19+ default): `object_type DataName(list_of_fields)`
with no `DATA` keyword and no square brackets. Concise data is always space
delimited.

Rules shared by both forms:

- The field list may span several lines. Fields are comma separated. A `//`
  comment anywhere in a line discards the rest of the line; blank lines are
  ignored.
- A value list row may span several lines; each new object starts on its own
  line, so a row is complete when it has one value per declared field.
- Strings quote with `"` and must quote when they contain spaces or commas.
  Empty quoted strings are valid values.
- Field names carry location suffixes, `variablename:location` (`BusNum:1` is
  the second bus of a branch); `:0` may be omitted. Simulator 19 renamed most
  located fields to concise names (`LineMW:1` became `MWTo`), and both
  generations of names appear in the wild.
- `<SUBDATA subobject_type> ... </SUBDATA>` blocks follow the value row of
  the object they belong to, inside the `{ }` body. Their interior format is
  fixed per subobject type; some hold free text (`PWCaseHeader`), some hold
  per line records (`CTGElement`, `LimitViol`, ...).
- Values may be special references (`"@field:loc:digits:decimals"`,
  `"&Objecttype 'keys' field"`); they are strings at the grammar level.

## Gap list: the reader before this work vs ACTIVSg200.aux

Baseline measured 2026-06-10 against the vendored fixture (Simulator 20
export, legacy headers, 22 DATA blocks over 21 object types). The reader
accepted the file and produced a 200 bus Network with no warnings, and the
result was wrong everywhere it could be:

1. **Multiline field lists truncated.** The parser required the whole
   `DATA (Object, [fields])` header on one line. 14 of the 22 blocks in the
   real export wrap their field lists (Bus declares 36 fields over 6 lines,
   Gen 62 over 10, Branch 55 and 76, Load 25, Shunt 23). Only the fields on
   the header's first line were mapped; every later field silently defaulted.
   Measured damage: all 246 branches parsed with R = X = 0 (the impedance
   fields sit past line one), every bus came back PQ (BusCat unread; the
   sibling case has 48 PV, 1 slack), vmax/vmin/area/zone all defaulted.
2. **ZIP load components unread.** The export carries no `LoadMW`; it writes
   `LoadSMW/LoadSMVR/LoadIMW/LoadIMVR/LoadZMW/LoadZMVR`. The reader looked up
   `LoadMW`, found nothing, and emitted 160 loads of 0 MW.
3. **Two Branch blocks conflated.** Lines (180 rows, 55 fields) and
   transformers (66 rows, 76 fields with tap/regulation data) are separate
   blocks; both were fed through one field mapping keyed to the writer's own
   13 field layout.
4. **16 of 21 object types dropped on the floor**: PWCaseInformation, Owner,
   Substation, Limit_Monitoring_Options_Value, LimitSet, RatingSetNameBus,
   RatingSetNameBranch, RatingSetNameInterface, Area, Zone,
   BalancingAuthority, Sim_Solution_Options_Value, PostPowerFlowActions,
   GICXFormer, Contingency, ContingencyElement. No retention, no warning.
   Contingency data (245 contingencies with 490 SUBDATA blocks) is the
   payload a transmission study cares about.
5. **SUBDATA unparsed.** `<SUBDATA>` tags inside a mapped block would have
   been read as value rows. The baseline file only carries SUBDATA in
   unmapped blocks (PWCaseInformation, Contingency), which is why the parse
   did not error.
6. **Comments unhandled.** `//` comments inside field lists or bodies were
   not stripped (the real file has them inside SUBDATA).
7. **Concise headers, CSV delimiting, DataName, create_if_not_found,
   SCRIPT sections, multiline value rows: unsupported.** None appear in the
   ACTIVSg exports, all are legal aux per the format guide.
8. **No structural fidelity.** Echoing back the same format relied entirely
   on the retained source; converting aux to aux through the typed model
   reduced 21 object types to the writer's 5.

Counts that survived the baseline: 200 buses, 49 generators, 160 loads,
4 shunts, 246 branches (180 + 66). The sibling `case_ACTIVSg200.m` carries
245 branches; reconciling the difference is part of the parity work.

## Mapping notes (established against the sibling exports)

- Complete case exports spread one object type over several DATA sections
  with complementary field groups (the 2016 Texas2000 export writes Bus
  twice, Gen three times, and a separate `Transformer` object for regulation
  fields). The reader merges sections by key fields: BusNum for buses,
  BusNum + device ID for loads/gens/shunts, bus pair + circuit for branches.
  `Transformer` sections only augment existing branches.
- Transformer records in Simulator 20 era exports carry impedance and tap
  under `:1` locations (`LineR:1`, `LineTap:1`); 2016 era exports use the
  bare names for everything. `LineTap` equals the MATPOWER tap convention
  (verified on all 66 ACTIVSg200 and 562 Texas2000 transformers).
- Loads are ZIP components (`LoadSMW/LoadIMW/LoadZMW`, ...). The typed model
  carries the sum at nominal voltage; nonzero I/Z components are kept in
  extras under their PowerWorld field names.
- PowerWorld stores no PV/PQ type: `BusSlack` marks the reference and PV is
  derived from in service generators. `BusVoltLim` is a YES/NO monitoring
  toggle, never a number; per rating set limits live in
  `BusVoltLimHigh:n`/`BusVoltLimLow:n` (empty in the ACTIVSg exports).
- Branch identity (circuit ID, device type) and substation coordinates ride
  in element extras under PowerWorld field names (`LineCircuit`,
  `BranchDeviceType`, `Latitude:1`, ...), so the aux writer reproduces them
  and cross format writers report them as extras.
- Generator has no extras map (a deliberate performance decision), so GenID
  and regulation fields are reachable only through the generic layer.
- Aux exports print f32 noise in some fields (`BusNomVolt`
  13.800000190734863); parity compares are approximate accordingly.

## Parity findings (vendored ACTIVSg200 set)

The vendored siblings are different case revisions: `.aux`/`.pwb` are a June
2018 pair, `case_ACTIVSg200.m` is October 2017, `.RAW` is May 2017. Identity
and impedance data agree (impedances to 5e-6, all 66 taps exact); the 2018
revision adds one line (82-64) absent from 2017, and the solved states and
load values differ between revisions. The June 2016 ACTIVSg2000 sibling set
(fetched) was exported in one day from one case and gives full value parity:
vm/va to 1e-6/1e-4, ZIP load totals vs MATPOWER Pd/Qd to the .m print
quantum, dispatch and branch values likewise. `powerio/tests/`
`powerworld_parity.rs` asserts all of this.

## The .pwb binary format (decode evidence)

Established by differential analysis of three lawfully obtained files against
their aux siblings, no PowerWorld software involved: ACTIVSg200.pwb
(Simulator 20 era, June 2018, same snapshot as the vendored aux),
Texas2000_June2016.pwb (June 2016, same day as its aux sibling), and
ACTIV_SG_2000_v19.pwb (April 2017). Every claim below was verified by exact
value match against the sibling aux on every record unless noted. Offsets are
from the field listed; integers and floats are little endian.

### Header (identical prefix in all three files)

| offset | type | value | meaning |
|---|---|---|---|
| 0x00 | u64 | 15000 | magic / format constant |
| 0x08 | u64 | 425 | writer format constant; 425 in every file this section decodes, but newer exports carry 483 (Texas7k 2021), 508 (saved as v21), 537, 550, or 551 (2022 era saves), and 425 alone does not pin the record layout (see the bus record flag words below) |
| 0x10 | u64 | 20 | format constant |
| 0x18 | 16 bytes | 0 | unknown |
| 0x28 | f64 | varies | Delphi TDateTime of the save (days since 1899-12-30); matches each file's export date |
| 0x30.. | | | case description block: a small count, then u32 length prefixed text paragraphs |

### Strings

Two encodings: u32 length prefixed byte strings (names, labels) and Pascal
ShortStrings (one length byte; device IDs, circuit IDs).

### Tables

Object tables appear in a fixed order matching the aux export: buses, loads,
generators, shunts, branches (lines and transformers interleaved, ordered by
bus), then the remaining object types. Each table is preceded by its record
count (u32; 200 appears at 0x328 in ACTIVSg200.pwb before the first bus
record, 49 before the generators, 246 before the branches) plus a short
table specific glue block.

### Bus record (validated on all 200 + 2007 buses of two vintages)

| field | type | notes |
|---|---|---|
| BusNum | u32 | record starts here |
| BusName | u32 string | |
| flags | u32 | a field presence bitmask, not a per file constant; see the census below |
| BusNomVolt | f32 | f32 storage explains the noise the aux prints (13.800000190734863) |
| AreaNum, ZoneNum, BANumber | u32 ×3 | |
| label | u32 string | "newbus 138" in the ACTIVSg cases |
| BusPUVolt | f64 | exact match with the aux |
| BusAngle | f64 | radians; aux prints degrees |
| tail | 85 bytes (2018) / shorter (2016) | constant across plain records within a file; contains DCLossMultiplier as f32 1.0 and flag bytes; undecoded. Records with flag bit 4 set insert a count prefixed list into the tail (u32 count, then 9 byte entries observed as u8 = 3, u32 number, u32 = 1; meaning undecoded) |

Substation coordinates are not in the bus record; they live with the
substation objects.

#### Bus record flag words (census of validated heads, leading 64 KiB)

The flag word is a field presence bitmask. Bit 5 set marks the Simulator 20
era record family (clear on the 2016 era family), bit 4 set marks the count
prefixed list in the record tail, bit 0 clear means one extra u16 before the
nominal kV (the generator buses: 49 such records in ACTIVSg200, which has 49
generators). The head layout through the solved voltage is identical across
every observed flag word; the tails differ.

| file | flag words seen |
|---|---|
| ACTIVSg200.pwb (June 2018) | 0x26 ×49, 0x27 ×151 (the full bus table) |
| Texas2000_June2016.pwb | 0x06 ×1, 0x07 ×425, 0x17 ×77 |
| ACTIV_SG_2000_v19.pwb (April 2017) | 0x26 ×139, 0x27 ×300, 0x36 ×1, 0x37 ×21 |

The reader decodes the plain tailed Simulator 20 era family (0x26/0x27) and
rejects the rest by census, naming the detected family. The v19 file walks
correctly through 1962 of its 2000 bus records with the 2018 layout; the
records with bit 4 set break the walk, which is why the whole file is
rejected until the tail lists are decoded.

### Load record (89 bytes fixed, validated on all 160)

u32 BusNum, ShortString LoadID, one byte (status, 0 observed for Closed),
then f32 values in per unit on the system base: LoadSMW/100 at +8 from
record start, LoadSMVR/100 at +12. Remainder undecoded (I/Z components are
zero in every available case).

### Generator record (variable length, validated on all 49)

u32 BusNum, ShortString GenID, then f32 per unit values at fixed offsets
from record start: MW setpoint +11, MVAr setpoint +15, MVRMax +19, MVRMin
+23, GenVoltSet +27 (p.u., scale 1), GenMVABase +31 (MVA, scale 1), MWMax
+35, MWMin +39, GenRMPCT +53. GenZR/GenZX as f64 near +147/+193. Record
length varies with embedded strings.

### Branch records (lines ~224 bytes, transformers ~384, interleaved)

u32 from bus, u32 to bus, u16 flags, then optionally a ShortString circuit
ID. Flags 0x00EE: circuit string present, followed by one byte (status
candidate), f32 LineR, f32 LineX, f32 LineC, f32 ratings/100 (LineAMVA at
+12 from LineX). Flags 0x00EF: no circuit string (PowerWorld's default
" 1"), R follows the flags directly. Transformer records carry the
regulation block at higher offsets: tap at +87 from record start, tap
limits +104/+108, step +122, nominal kV pair +169/+173, XFMVABase +177.
The flags word is the Delphi optional field bitmask; only the observed
values are accepted and anything else is a loud error.

Sibling print precision matters for transformer parity: the aux transformer
Branch section prints impedances at 6 decimals while the line section prints
the f64 widening of the stored f32 at 20 decimals. The binary stores the
full f32 either way, confirmed by the RAW sibling's 6 significant digits:
transformer (15,14) R reads 0.000637329 from the binary, prints 6.37329E-4
in the RAW and 0.000637 in the aux and the .m. Parity tests therefore
compare transformers against the aux at its print quantum and against the
RAW at full precision.

### Open questions (inventoried, not guessed)

- Status encodings (every device in the available 200 bus case is Closed);
  the 2016 Texas case carries open devices and pins them.
- The bus record tail bytes and the branch record interiors beyond the
  fields above.
- Table glue blocks between count and first record.
- Substation, area/zone names, contingency tables: present after the
  branches, undecoded in this pass.

## Object inventory of ACTIVSg200.aux

| object | rows | fields | notes |
|---|---|---|---|
| PWCaseInformation | 1 | 1 | PWCaseHeader SUBDATA holds the case description |
| Owner | 1 | 3 | |
| Substation | 111 | 8 | latitude/longitude per substation |
| Limit_Monitoring_Options_Value | 1 | 2 | |
| LimitSet | 1 | 19 | |
| RatingSetNameBus | 4 | 3 | |
| RatingSetNameBranch | 15 | 3 | |
| RatingSetNameInterface | 15 | 3 | |
| Bus | 200 | 36 | |
| Gen | 49 | 62 | |
| Load | 160 | 25 | ZIP components |
| Branch (lines) | 180 | 55 | |
| Branch (transformers) | 66 | 76 | tap, regulation fields |
| Shunt | 4 | 23 | |
| Area | 1 | 21 | |
| BalancingAuthority | 200 | 7 | |
| Zone | 7 | 6 | |
| Sim_Solution_Options_Value | 69 | 2 | |
| PostPowerFlowActions | 1 | 1 | |
| GICXFormer | 66 | 15 | ground ohms |
| Contingency | 245 | 32 | 490 SUBDATA (CTGElement, LimitViol) |
| ContingencyElement | 245 | 11 | |
