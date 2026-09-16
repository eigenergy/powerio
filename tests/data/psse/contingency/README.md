# PSS/E contingency analysis fixtures

Every file here is original to this repository. Siemens does not publish the
normative `.con`, `.sub`, or `.mon` grammar, so the statements these files
state were drawn from public contingency analysis files and from question
threads about them. The fixtures carry no content from those sources: each one
is written here, and the sources below are cited as evidence that a spelling
occurs in real files.

| File | Bytes | What it covers |
| --- | --- | --- |
| `psse35_generated.con` | 141 | The header PSS/E 35 writes (`/PSS(R)E` stamp, `COM` banner), one automatic specification, a file `END`. |
| `explicit_mixed.con` | 934 | Explicit cases: quoted, bare, and apostrophe names; every action the grammar reads; synonyms, lowercase keywords, tab indentation, a mid-file `COM` line. |
| `no_file_end.con` | 203 | The corpus layout: `BUS   1001` column spacing, no file `END`. |
| `automatic_skip.con` | 175 | Automatic specifications for branches, units, and ties, `3WLOWVOLTAGE`, a `SKIP` block, and one line after the file `END`. |
| `tara_extensions.con` | 367 | Statements only the TARA solver reads: a `//` comment, `BUSNUMBERS`, `BRANCHNAMES`, bus-name mode, a padded quoted circuit, a trailing `/` comment, a nested dispatch block, and the `DEFAULT DISPATCH` and `DEFAULT DISPATCH DOWN` block openers. |
| `resolve_cases.con` | 1452 | One case per binding rule: each branch circuit, a statement in the reverse orientation, a missing branch, each machine id, a wrong machine id, the fixed shunts at a bus and one of them by id, a switched shunt, the loads at a bus, a three winding transformer stated out of winding order, a bus disconnection, a load change, an empty case, and one statement kept as text. |
| `resolve_v33.raw` | 4625 | The network `resolve_cases.con` resolves against: five buses, machines `1`/`2` on bus 1 and `3`/`1` on bus 2, parallel circuits `1` and `2` between buses 1 and 2, a branch stored 3-1, a branch on circuit `BL`, an out of service branch, two fixed shunts and one switched shunt on bus 3, two loads on bus 4, and one three winding transformer on buses 2, 4, and 5. |
| `expand.con` | 348 | One automatic specification per target, `3WLOWVOLTAGE`, a `DOUBLE`, a `SKIP` block, one explicit case, and a specification naming a subsystem no `.sub` file states. |
| `psse35_area.sub` | 124 | The header PSS(R)E 35 writes, one `AREA` subsystem, the subsystem `END` and the file `END`. |
| `selectors.sub` | 622 | Every selector spelling: `BUS` lines, `BUSES a b`, `AREAS a b`, `ZONE`, `OWNER`, `KVRANGE`, a bare name, the `SYSTEM` synonym, two named `JOIN` groups, two `JOIN` groups with no name, a one line `SUBSYSTEM ... END`, a TARA `SCALE` line, tab and space indentation, a blank line, and no file `END`. |
| `generated.mon` | 601 | The header PSS(R)E 34 writes, `VOLTAGE RANGE` and `VOLTAGE DEVIATION` on a subsystem, `BRANCHES IN` with `3WLOWVOLTAGE`, `LINES IN`, `TIES FROM`, `ALL BUSES` with one and with two deviation values, the `BUS`, `AREA`, `ZONE`, `OWNER`, and `KV` scopes, and two file `END`s. |
| `blocks.mon` | 330 | The block forms: a `MONITOR BRANCHES` block with mixed indentation, an absent circuit, and a branch the network does not hold; two `MONITOR INTERFACE` blocks with and without `RATING`; a statement naming an unknown subsystem; one unrecognized line; one file `END`. |
| `select_v33.raw` | 4905 | The network `selectors.sub`, `generated.mon`, `blocks.mon`, and `expand.con` work over: six buses across three areas, four zones, three owners, and four base kV levels; parallel circuits between two buses; two ties, one of them out of service; four machines, one out of service; and two three winding transformers whose lowest voltage windings sit in different areas. |

## Evidence

| Source | What it shows |
| --- | --- |
| [IdahoLabUnsupported/PSSE_for_Human_factor_research](https://github.com/IdahoLabUnsupported/PSSE_for_Human_factor_research) `WOA.con` | The `/PSS(R)E` and `COM` header PSS/E 35 writes, and `SINGLE BRANCH IN SUBSYSTEM` with a quoted subsystem name. |
| [jbarberia/DespachoDeSeguridadPSSE](https://github.com/jbarberia/DespachoDeSeguridadPSSE) `IEEE14.con` | Explicit cases with bare names and `OPEN BRANCH FROM BUS i TO BUS j`. |
| [Guzman-Dufrechou/Flujo_DC-uy](https://github.com/Guzman-Dufrechou/Flujo_DC-uy) `IEEE5bus.con` | `REMOVE MACHINE id FROM BUS i` beside branch openings in one file. |
| [GOCompetition/Validation](https://github.com/GOCompetition/Validation) `case14.con` | Machine and branch cases generated per element, one action per case. |
| [ORNL/ExaGO](https://github.com/ORNL/ExaGO) `case9_pw.con` | A generated file whose case names encode the element the case outages. |
| The PSS/E example set `savnw.con`, via [Power-Agent/PowerMCP](https://github.com/Power-Agent/PowerMCP) | The example file Siemens ships: `CIRCUIT` on a branch line, a file `END`. |
| [PowerGem/VSCodeExtension](https://github.com/PowerGem/VSCodeExtension) `examples/Contingency.con` | The TARA statements: `BUSNUMBERS`, `BUSNAMES`, `BRANCHNAMES`, bus names in place of numbers, `DEFAULT DISPATCH` blocks, `//` comments. |
| [psspy.org question 9168](https://psspy.org/psse-help-forum/question/9168/) | `SKIP` blocks and the `i TO j CKT c` lines inside them. |
| `ACTIVSg2000.con` (machine-local corpus, see `tests/data/local_psse_contingency_corpus.tsv`) | The column spacing `BUS   1001`, case names holding an apostrophe, a file with no trailing `END`, and an empty case. |
| `ACTIVSg2000.RAW` (machine-local corpus, same manifest, label `activsg2000_raw`) | The case `ACTIVSg2000.con` names, so every statement of a 3875 case file is resolved against a network of the size it was written for. |
| [IdahoLabUnsupported/PSSE_for_Human_factor_research](https://github.com/IdahoLabUnsupported/PSSE_for_Human_factor_research) `WOA.sub` | The `SUBSYSTEM 'name'` block with an `AREA` selector and the two `END`s, as the Config File Builder writes it. |
| [jbarberia/DespachoDeSeguridadPSSE](https://github.com/jbarberia/DespachoDeSeguridadPSSE) `IEEE14.sub`, `IEEE14.mon` | A subsystem file and a monitored element file beside the `.con` of the same case. |
| [Guzman-Dufrechou/Flujo_DC-uy](https://github.com/Guzman-Dufrechou/Flujo_DC-uy) `IEEE118bus.sub` | `BUS` selectors listed one per line inside a subsystem. |
| [GOCompetition/Validation](https://github.com/GOCompetition/Validation) `All_SDET.SUB` | A generated subsystem file over a large case. |
| [mjc-brito/universityProjects](https://github.com/mjc-brito/universityProjects) `full.sub` | A subsystem whose body states several selector families. |
| [ibrahim-siali](https://github.com/ibrahim-siali) `ieee14.sub`, [piq-energy/power-system-benchmark](https://github.com/piq-energy/power-system-benchmark) `smallsystem_sub.sub` | Hand written subsystem files: bare names and loose indentation. |
| The PSS/E example set `savnw.sub` and `savnw.mon`, via [Power-Agent/PowerMCP](https://github.com/Power-Agent/PowerMCP) | The example files Siemens ships: the `MONITOR BRANCHES` and `MONITOR INTERFACE` block forms, and `MONITOR VOLTAGE RANGE` on a bus, an area, and a zone. |
| [jbarberia/pssetools](https://github.com/jbarberia/pssetools) `parse_sub.py` | The selector keywords and the rule that a group unions within a selector family and intersects across families. |
| [PowerGem/VSCodeExtension](https://github.com/PowerGem/VSCodeExtension) `syntaxes/sub.tmLanguage.json` | The keyword list a `.sub` editor highlights, `JOIN` and the TARA-only statements among them. |
| [psspy.org question 1700](https://psspy.org/psse-help-forum/question/1700/) and [question 2132](https://psspy.org/psse-help-forum/question/2132/) | `MONITOR VOLTAGE RANGE SUBSYSTEM` and `MONITOR TIES FROM SUBSYSTEM` as users write them. |
| [3phaseee.com](https://www.3phaseee.com/) PSS/E notes | The `KVRANGE` selector, the `MONITOR VOLTAGE DEVIATION` statement, and the group semantics stated in words. |
