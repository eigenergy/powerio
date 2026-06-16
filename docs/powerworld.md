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
  generations appear in exported files.
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
  bare names for everything, tap included. `LineTap` equals the MATPOWER
  tap convention (verified on all 66 ACTIVSg200 and 562 Texas2000
  transformers, every one of which stores 1.0, so the off nominal bare
  name path is pinned by a synthetic test rather than parity).
- 2022 era exports (Simulator 21+, the Hawaii40 set) write a third naming
  generation: concise headers with `Number`/`Name`/`NomkV`/`Vpu`/`Vangle`
  on buses, `ID`/`Status`/`SMW`/`SMvar` on devices,
  `BusNumFrom`/`BusNumTo`/`Circuit`/`R`/`X`/`B`/`LimitMVAA` on branches,
  and `Rxfbase`/`Tapxfbase` on the transformer section. The mapping reads
  all three generations through alias lists, and the section merge keys
  carry the same aliases so the dual Branch sections join correctly. The
  Hawaii40 pwb parity test cross validates the vocabulary: both readers
  must agree value by value for it to pass.
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

Established by differential analysis of three lawfully obtained files, no
PowerWorld software involved: ACTIVSg200.pwb (Simulator 20 era, June 2018,
same snapshot as the vendored aux), Texas2000_June2016.pwb (June 2016, same
day as its aux sibling), and ACTIV_SG_2000_v19.pwb (April 2017, validated
against the published ACTIVSg2000 case with the snapshot deltas pinned in
the parity test). Every claim below was verified by value match against a
sibling on every record unless noted. Offsets are from the field listed;
integers and floats are little endian.

### Header (identical prefix in all three files)

| offset | type | value | meaning |
|---|---|---|---|
| 0x00 | u64 | 15000 | magic / format constant |
| 0x08 | u64 | varies | writer format constant. Decoded constants: 338, 368, 425, 483, 508, 537, 550, 551. Other older samples carry 196, 191, 134, or a different header shape. The constant does not pin the record layout; record flags and table anchors do. |
| 0x10 | u64 | 20 | format constant |
| 0x18 | 16 bytes | 0 | unknown |
| 0x28 | f64 | varies | Delphi TDateTime of the save (days since 1899-12-30); matches each file's export date |
| 0x30.. | | | case description block: a small count, then u32 length prefixed text paragraphs |

### Strings

Two encodings: u32 length prefixed byte strings (names, labels) and Delphi
ShortStrings (one length byte; device IDs, circuit IDs). Some ShortString
fields have a fixed capacity: the branch circuit ID and the generator ID are
`string[2]` (one length byte plus a fixed two byte text area, so a one
character value leaves an unused byte), while load IDs are stored variable
length. The v19 file's parallel circuit records (the first in the corpus
with two character circuit IDs) established the fixed capacity; the byte
once assumed to be the branch status was the unused capacity byte.

### Tables

Object tables appear in a fixed order matching the aux export: buses, loads,
generators, shunts, branches (lines and transformers interleaved, ordered by
bus), then the remaining object types. Each table is preceded by its record
count (u32; 200 appears at 0x328 in ACTIVSg200.pwb before the first bus
record, 49 before the generators, 246 before the branches) plus a short
table specific glue block.

### Bus record (validated on all 200 + 2007 + 2000 buses of three files)

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

Full file censuses: June2016 carries 0x06 ×273, 0x07 ×1544, 0x16 ×9,
0x17 ×181 over its 2007 buses (bit 0 clear on the 282 generator buses);
v19 carries 0x36/0x37 on 22 of its 2000. The reader decodes both families:
the head parses identically, the tails (57 bytes in the 2016 family, 85 in
the 2018 one, plus the bit 4 lists) are skipped by the record resync.

Newer writers widen the family with bits 6 and 8: Texas7k_20210804.PWB
(header constant 483) carries 0x66 ×481, 0x166 ×187, 0x167 ×6049 over its
6717 buses, and the current era ACTIVSg2000.PWB export (header constant
425!) carries 0x66/0x67/0x166/0x167/0x177. The head layout still matches
through the solved voltage, with bits 0 and 8 varying per record. Bit 6
looked file constant until the Texas7k v21 resave, which clears it on
exactly one of its 6717 records, the slack bus; the bit 6 records carry a
location string block (city, county, state text) in their tails that the
slack record lacks, so bit 6 is a per record presence bit like 0, 4, and
8, and the reader's family rule treats it as one. The 2030 build adds the
other tail surprise: its bit 4 count prefixed lists run to 149 nine byte
entries (1341 bytes) on one record, past the bounded resync window, so a
bit 4 bus record extends the scan to the buffer end exactly as a bit 4
branch record does. The 39 bus sample case (header 425) shows no
recognized bus record layout at all in a 44 KiB file.

An earlier draft of this section read the Texas7k generator table as "the
leading u32 equals the aux BusNum on roughly three quarters of the
records, the rest store a nearby bus and regroup unit IDs", and concluded
node level storage was blocking the 483 decode. The actual story is a
four byte field insertion: the 2021 era generator record carries the
regulated bus number as a second u32 after the terminal bus, and the
probe had keyed the record start one field late, reading the regulated
bus as the terminal bus. Plants regulating a remote bus (184 of 731
machines) are what made "a quarter" of the records look wrong. With the
boundary re-fit, every Texas7k table decodes against the same day aux:
buses (6717), loads (5095, values exact at f32 precision), generators
(731, every field plus the in service bit), shunts (634), branches
(9140, every identity, impedance, rating, and tap). The generator record
layout is in its own section below.

The TAMU repository sets re-downloaded in June 2026 supply what that fit
was missing, same source aux siblings for the bit 6/8 family: ACTIVSg500
(header 425), the published ACTIVSg2000 set (header 425), and Hawaii40
(header 508). With the flag masks widened to admit bits 6 and 8 (both
leave the bus head layout untouched; their fields live in the undecoded
tails), the two ACTIVSg2000 current era exports decode end to end, and
the published set export carries full value parity against its same set
aux on every decoded quantity (the test next to the other vintages'). ACTIVSg500's branch records with flag bit 4
append large tail structures: per bus f64 vectors (a u32 count equal to
the bus count is visible inside), ascending bus number arrays, and
contingency label text, up to 406 KiB on one record. The reader handles
them by extending the resync scan to the buffer end after a bit 4 branch
record (the ~90 byte structural gauntlet keeps blob bytes from forging a
record; two forged record heads were found inside blobs and both fail
it). With that rule ACTIVSg500 decodes with full value parity against
its aux. Hawaii40 (2022, header constant 508) decodes with full parity
the same way, which is the evidence admitting the 508 header era; its
aux uses the 2022 concise vocabulary (see the mapping notes).

The header constants past 508 fell with the Texas7k saves. The v21
resave (508) needed the bus bit 6 family fix and a 52 byte bus table
glue plus an 86 byte generator table glue (string metadata sits between
the count word and the first record); with those, the whole sibling
family decodes through one record model set: the v22 save (551), the
2030 build (550, 7132 buses with the pres bit 5 generator records and
the long bit 4 bus lists), the 2030 v22 save (537), and the November
2021 scenario snapshot (537). v21 and v22 carry committed parity tests
against their same day 2022 aux; the 2030 saves were value checked by
strict offline alignment against their aux (1058 of 1058 generators,
bus, regulated bus, ID, MW, and status each) and their name keyed aux
export (BusName_NomVolt keys instead of BusNum) keeps them out of the
committed aux comparison until the aux reader learns that vocabulary.

Header 554 keeps the same bus, load, shunt, and branch heads, but widens the
load table glue to 104 bytes and uses a regulated generator record without
the 2021 era presence byte: terminal bus, regulated bus, fixed ID, two zero
bytes, the shared f32 generator block, then the same status/RMPCT tail. Its
branch table count can include one trailing non branch record; the accepted
branch run still has to pass the normal branch record validation and end check.

### Load record (validated on all 160 + 1417 + 1350 + 5095 loads of four files)

u32 BusNum, variable length ShortString LoadID, one undecoded byte, then
f32 values in per unit on the system base: LoadSMW/100, LoadSMVR/100.
Remainder undecoded (I/Z components are zero in every available case). The
byte after the ID is 0x00 in every 425 era record and 0x01 in every 483
era one while both auxes mark every load Closed, so it is not a status
byte; an earlier draft treated it as one. The 2021 era layout is otherwise
identical: the Texas7k exports and every later save of the family carry
per load value parity against their auxes at f32 precision.

### Generator record (validated on all 49 + 282 + 545 machines of three files)

u32 BusNum, GenID as ShortString[2] (fixed three byte field), then flag
bytes whose count varies, then eight consecutive f32 per unit values
anchored at +9 or +10 (2016/2017 exports; the gap varies per record) or +11
(2018) from the record start: MW setpoint, MVAr setpoint, MVRMax, MVRMin,
GenVoltSet (p.u., scale 1), GenMVABase (MVA, scale 1), MWMax, MWMin. The
voltage setpoint and MVA base ranges pick the anchor per record. In the
2018 file also verified: GenRMPCT at +53, GenZR/GenZX as f64 near
+147/+193. Record length varies with embedded strings; the status byte is
unlocated within the flag bytes (every machine in these files is Closed).

### Generator record, 2021 era (validated on 731 ×3 + 1058 ×2 machines of five files)

The 2021 era writer (Texas7k export and every later save of the family)
inserts the regulated bus and pins the layout to fixed offsets:

| offset | type | field |
|---|---|---|
| +0 | u32 | BusNum (terminal bus) |
| +4 | u32 | GenRegNum (the inserted field; equals BusNum except on the 184 remote regulating machines) |
| +8 | ShortString[2] | GenID |
| +11 | u8 | constant 1 |
| +12 | u8 | varies per record (7 through 37 observed), undecoded |
| +13 | u8 | presence byte: bit 0 inserts an f32, bit 1 one byte (the 2021 export), bit 5 another f32 (the 2030 build); the inserted values are undecoded |
| +14.. | f32 ×8 | the same block as the older eras: MW setpoint, MVAr setpoint, MVRMax, MVRMin, GenVoltSet, GenMVABase, MWMax, MWMin |
| block +32 | u8 | constant 0 |
| block +33 | u8 | status: 9 in service, 8 open; bit 0 validated against the aux on 637 Closed + 94 Open machines per file, the corpus's only located device status |
| block +34 | f32 | GenRMPCT (100.0 on every record), a structural anchor |

The record tail (undecoded) carries the piecewise cost curve as f32
pairs and per record strings; tail lengths run 238 to 274 bytes in the
2021 export.

### Branch records (validated on all 246 + 3043 + 3202 branches of three files)

u32 from bus, u32 to bus, u16 flags, then in order:

- circuit ID as ShortString[2] (three byte field), unless flag bit 0 says
  it is omitted (PowerWorld's default " 1" applies);
- f32 LineR, LineX, LineC, LineG (per unit);
- inline per unit rating f32s: LineAMVA, LineAMVA:1 when flag bit 1 is set
  (the 2018 and v19 exports), plus LineAMVA:2 when it is clear (June 2016);
- a constant u32 tag = 12, a structural anchor every record carries;
- eleven f32 slots, zero in every available case (presumably further rating
  locations; left undecoded);
- one zero byte, then the kind byte: 0x01 line, 0x00 transformer, with the
  transformer's LineTap as f32 immediately after.

The flags word is the Delphi field presence bitmask, base bits 0x6C: bit 0
omits the circuit ID, bit 1 selects two inline ratings instead of three,
bit 4 appends a count prefixed list to the record tail (as in the bus
records), and bit 7, set on every record of every 425/508 era export, is
per record in the 2021 era: the Texas7k export clears it on 7135 of its
7173 lines while setting it on every transformer and the remaining 38
lines, with the head layout identical either way (its field is in the
undecoded tail; the transformer tails carry a regulation block with the
tap limits and the to bus echoed). Observed words: 0xEC/0xFC (2016,
2899 + 144 records), 0xEE/0xEF (2018 and v19), 0xFE/0xFF (v19, 195 + 5),
0x6C and 0xEC/0xED (Texas7k: 7135 + 2000 + 5). In the 2018 file also verified,
within the transformer tail: tap limits +104/+108, step +122, nominal kV
pair +169/+173, XFMVABase +177 from the record start. The branch status is
unlocated (every available record is in service); an earlier draft of this
section took the circuit ID's unused capacity byte for the status and the
byte before the kind byte for the kind, which made every 2018 line read as
a zero tap transformer in extras until the v19 parallel circuits exposed
both.

Sibling print precision matters for transformer parity: the aux transformer
Branch section prints impedances at 6 decimals while the line section prints
the f64 widening of the stored f32 at 20 decimals. The binary stores the
full f32 either way, confirmed by the RAW sibling's 6 significant digits:
transformer (15,14) R reads 0.000637329 from the binary, prints 6.37329E-4
in the RAW and 0.000637 in the aux and the .m. Parity tests therefore
compare transformers against the aux at its print quantum and against the
RAW at full precision.

### Shunt record (validated on all 4 + 41 + 154 shunts of three files)

u32 BusNum, ShortString ShuntID, with the nominal MVAr as f32 in per unit
at +24 from the record start in every vintage. The slot at +20 is 0.0 in
the Simulator 20 era files but 0.99 in the 2016 export (a regulation
target, not a power), so the nominal MW slot is unlocated; every available
case stores zero shunt MW and the reader sets G = 0.

### Open questions (inventoried, not guessed)

- Status bytes: the 2021 era generator status is located and validated
  (bit 0 of the byte one past the f32 block, against 94 open machines);
  every other device in every available case is Closed/in service, so no
  other status offset is validated and those devices read as in service.
  Whether the older era generator records carry the same byte after their
  block is untested for the open state (no 425 era case has one).
- The meaning of the bit 4 tail lists (u32 count, then 9 byte entries
  observed as u8 = 3, u32 number, u32 = 1) and of the constant u32 12 tag
  in branch records.
- The eleven zero f32 slots after the branch rating tag, and the bus and
  branch record tail bytes beyond the fields above (the 2021 era
  transformer tails hold a regulation block and the generator tails the
  piecewise cost curve, both undecoded; branch flag bit 7's field).
- The 2021 era generator record's presence gated values (bits 0 and 5
  insert f32s, bit 1 a byte; observed values 4.0/5.0 and 1.17) and its
  constant +11/+12 bytes.
- The gen record's variable flag byte gap (+9/+10 within one 2016 file).
- Whether load IDs are also fixed capacity (every observed load ID already
  fills two characters or parses either way).
- Table glue blocks between count and first record (the v21 resave's bus
  and generator glues carry 52 and 86 bytes including string metadata).
- Substation, area/zone names, contingency tables: present after the
  branches, undecoded in this pass.

## The .pwd display format: substation coordinates

`.pwd` files are display artifacts, not network cases, so `parse_file`
rejects them with a pointer to the display API. Use
`parse_display_file(path, None)` / `parse_display_bytes(bytes, "pwd")` for
the generic surface, or the lower level PowerWorld helpers
`parse_pwd_file`, `parse_pwd_display`, and `parse_pwd`. The display result
is `DisplayData::PowerWorld(PwdDisplay { canvas_width, canvas_height,
stamp, substations })`; Python returns `DisplayData("powerworld",
PwdDisplay(...))`.

The `.pwd` decoder reads one subset of the display sibling, the substation
symbols, established by differential analysis of seven files spanning the
June 2016 through 2022 writer eras. Every other drawing object type (buses,
branch pies, transmission lines, field labels), the palettes, fonts,
layers, and the substation record style tails stay undecoded.

Header: u32 = 50, two u16 canvas dimensions, then a fixed shape block. The
u32 at offset 22 is a per file stamp that every drawing object record
repeats at +18 — the anchor the record scan keys on. A correction to the
earlier probe notes: the type name list behind "Previous Select By
Criteria Set Used" in 2017+ saves is the object type list of that dialog's
last use (UI state), not a registry of the record types in the file
(ACTIVSg200.pwd lists only DisplaySubstation yet draws eight plus types);
the decoder takes nothing from it, and the June 2016 save has none.

Two structures carry substations when the display includes substation symbols:

- The identity table, behind the file's only `ff ff ff ff 3d 0f` sequence
  (sentinel plus table tag 0x0f3d): records of u32 number, the same u32
  again, u32 length, name, 0x02, terminated exactly by the next
  `ff ff ff ff`. Display order, not case order. A bus identity table (tag
  0x0f3c, no coordinates) directly precedes it, undecoded.
- The DisplaySubstation drawing records: u16 type tag, f32 x, f32 y at
  +2/+6 (echoes), u32 flag, zeros, u16 0x000a, the header stamp at +18,
  f64 x at +22, f64 y at +30, f64 0.0, then a style tail holding a digit
  string (1 to 4 characters, shifts later fields), `ff ff ff ff`, a marker
  byte, the u32 substation number, a 4 x f64 bounding box, and font
  fields. Record lengths run 139 to 162 by era. The type tag (0x27e2,
  0x27e3 observed) and the marker (0x03, 0x07) drift across writer eras,
  so the reader keys on structure instead: stamp echo at +18, the f32/f64
  dual coordinate equality, magnitude in [1, 1e7), and a marker plus
  number link to every identity row in table order. Two real decoy groups
  force that gauntlet: the era B substation field label group (same
  count, different order; positional pairing scores r² 0.01 against the
  oracle) and the Texas2016 interleaved label group (marker 0x05). Both
  fail the link check; if several groups ever pass, the reader rejects
  rather than guesses.

Some display files have a valid PowerWorld display header but no substation
identity table. Those decode as `PwdDisplay` with an empty `substations` list;
bus symbols and other drawing objects remain undecoded.

The coordinates are diagram positions, not geography (no probed file
stores latitude or longitude; needle scans came back empty). The auto
generated layouts equal x = k·longitude, y = k·merc(latitude) with
merc(lat) = degrees(ln(tan(45° + lat/2))): Hawaii40, never hand edited,
reproduces it to f64 rounding (max residual 2.9e-11) and pins
k = 535.8160803622592; the TAMU layouts carry hand moved symbols (median
residual 0.006 to 43 units across files) and the June 2016 writer used a
different transform entirely. The reader therefore exposes x/y as stored;
projecting back to geography is the consumer's choice, and consumers who
want coordinates as data should read the aux Substation latitude and
longitude instead.

Per file evidence (powerio/tests/powerworld_pwd.rs asserts the committed
subset; the rest ran in the scout probes):

| file | substations | aux (number, name) match | x vs longitude r² | y vs merc(lat) r² |
|---|---|---|---|---|
| ACTIVSg200 (vendored) | 111 | 111/111 | 0.99992 | 0.99980 |
| Illinois display mislabeled ACTIVSg2000.pwd | 111 | 111/111 | 0.9972 | 0.9951 |
| ACTIVSg500 (local) | 208 | 208/208 | 0.99999 | 0.999995 |
| ACTIVSg2000 published set (local) | 1250 | 1250/1250 | 0.999999 | 0.999999 |
| ACTIV_SG_2000_v19 (fetched) | 1250 | 1248/1250 vs the published aux (vintage skew) | 0.9935 | 0.9961 |
| Texas2000 June 2016 (fetched) | 1500 | 1500/1500 | 0.99962 | 0.99966 |
| Hawaii40 (local, 2022) | 31 | 31/31 | exact | exact |

## Coverage matrix

The committed `powerworld_corpus.rs` test pins the vendored and fetched rows.
Local only rows live outside the repository; machine specific paths belong in
the gitignored local manifest. Tiers: decoded with parity, classified and
rejected, out of scope.

| file | provenance | header | bus flags | oracle | verdict | counts |
|---|---|---|---|---|---|---|
| ACTIVSg200.pwb | vendored (TAMU) | 425 | 0x26/0x27 | same snapshot aux + 2017 RAW | decoded, parity on every quantity | 200 buses, 246 branches |
| Texas2000_June2016.pwb | fetched (TAMU) | 425 | 0x06-0x17 | same day aux | decoded, parity on every quantity | 2007 buses, 3043 branches |
| ACTIV_SG_2000_v19.pwb | fetched (powerworld.com) | 425 | 0x26-0x37 | published case .m, deltas pinned | decoded, parity | 2000 buses, 3202 branches |
| RTS-GMLC.PWB | fetched (GridMod/RTS-GMLC @3ece0d3) | 425 | 0x06/0x07 | same commit .m + .RAW | decoded, parity | 73 buses, 120 branches |
| Texas7k 2021 export | local only | 483 | 0x66-0x167 | same day aux + .m | decoded, parity on every quantity including the 94 open machines and the .m topology | 6717 buses, 9140 branches |
| Texas7k saved as v21 | local only | 508 | 0x26-0x167 | same day 2022 aux | decoded, parity (the bus bit 6 evidence) | 6717 buses, 9140 branches |
| Texas7k saved as v22 | local only | 551 | 0x26-0x167 | same day 2022 aux | decoded, parity | 6717 buses, 9140 branches |
| Texas7k 2030 build | local only | 550 | 0x66-0x167 | aux, offline strict alignment (name keyed export) | decoded, counts committed; 1058/1058 generators value checked offline | 7132 buses, 9555 branches |
| Texas7k 2030 saved as v22 | local only | 537 | 0x66-0x167 | aux, offline strict alignment | decoded, counts committed | 7132 buses, 9555 branches |
| Texas7k 2021 scenario snapshot | local only | 537 | 0x66-0x167 | same grid as the 2021 export | decoded, counts match the 2021 case | 6717 buses, 9140 branches |
| IEEE 14 PowerWorld save | local only | 554 | 0x06/0x07 | standard IEEE 14 topology | decoded offline | 14 buses, 20 branches |
| 39 bus sample case | local only | 425 | 0x06/0x07 | RAW/EPC sibling | decoded; counts, totals, and branch topology match | 39 buses, 46 branches |
| 118 bus sample case | local only | 338 | 0x06 family | RAW/EPC sibling | decoded; counts, totals, and branch topology match | 118 buses, 186 branches |
| 12 bus course case | local only | 134 | — | — | rejected: header constant | |
| 10 bus sample case | local only | 196 | — | — | rejected: header constant | |
| 3 bus sample case | local only | pre 425 shape | — | — | rejected: header words | |
| ACTIVSg500 export | local only | 425 | 0x66-0x177 | same set aux | decoded, parity on every quantity | 500 buses, 599 branches |
| ACTIVSg2000 published set export | local only | 425 | 0x66-0x177 | same set aux | decoded, parity on every quantity | 2000 buses, 3206 branches |
| ACTIVSg2000 current era export | local only | 425 | 0x66-0x177 | published case | decoded, counts verified; value parity test pending | 2000 buses, 3206 branches |
| Hawaii40 2022 export | local only | 508 | 0x66-0x167 | same set aux (2022 vocabulary) | decoded, parity on every quantity | 37 buses, 89 branches |
| 12 bus course case saved as v21 | local only | 508 | — | — | decoded, counts verified | 12 buses, 18 branches |
| .pwd display files | local/fetched | 50 | — | sibling aux Substation latitude/longitude | substation coordinates decoded, matched 1-1 (see the .pwd section) | 111 through 1500 substations across seven files |

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
