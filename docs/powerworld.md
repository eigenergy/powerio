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
