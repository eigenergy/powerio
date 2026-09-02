# CGMES contribution audit

Mohamed Numair implemented the first CGMES reader and writer on
`core/cgmes`:

- [`a26b60452a853126f84b9c793e2cdc603ec43081`](https://github.com/eigenergy/powerio/commit/a26b60452a853126f84b9c793e2cdc603ec43081)
  added CGMES input.
- [`11163b32faf87dbe8a64a94a388a771e36c6df8f`](https://github.com/eigenergy/powerio/commit/11163b32faf87dbe8a64a94a388a771e36c6df8f)
  added CGMES output.

The PowerIO 1.0 implementation incorporates that work. This audit records how
each part of the two commits appears in the final code.

## Reader

| Original behavior | PowerIO 1.0 status |
|---|---|
| Detect CGMES 2.4.15 from CIM16 namespaces, including observed vendor URI years | **Retained.** The reader accepts CIM16 spellings and records the detected release. |
| Detect CGMES 3.0 from the CIM100 namespace | **Retained.** |
| Classify profile files from `md:FullModel` profile URIs | **Retained and tightened.** EQ and TP are required. SSH, SV, EQBD, and TPBD are optional inputs with explicit handling. |
| Read `md:FullModel` metadata | **Retained and tightened.** Scenario time, description, and modeling authority data use the existing case metadata. Identity, creation time, version, dependencies, unknown properties, conflicts, and nested RDF/XML that have no source neutral home receive grouped diagnostics. |
| Merge `rdf:ID` definitions with `rdf:about` property fragments | **Retained and tightened.** The RDF graph checker separates model headers from CIM objects, rejects duplicate definitions and dangling references, and checks normalized archive names. |
| Read a profile directory or one merged XML file | **Improved.** `Source` also reads one ZIP containing profiles and a directory containing profile ZIPs. The same path works for files, directories, and memory input. |
| Skip DL, GL, and DY profile data with warnings | **Retained as precise diagnostics.** These profiles are outside the 1.0 electrical mapping and never disappear silently. |
| Use TP `TopologicalNode` records as the balanced buses | **Retained and improved.** The source neutral model also keeps substations, voltage levels, connectivity nodes, busbar sections, terminals, switches, containment, and both bus breaker and node breaker relationships. The balanced buses remain the calculation view. |
| Read terminal connectivity through direct `TopologicalNode` references or `ConnectivityNode.TopologicalNode` | **Retained and improved.** Terminal identities, sequence numbers, service status, configured nodes, calculated buses, and disconnected associations survive fresh output where the target can carry them. |
| Read `SvVoltage`, `SvPowerFlow`, `SvTapStep`, and `SvShuntCompensatorSections` | **Retained and improved.** The reader keeps solution quantities separately from operating assignments, includes `SvStatus`, and diagnoses authority data that cannot be assigned to one projected calculation bus. |
| Map `ACLineSegment`, line charging, and current limits | **Retained and improved.** Names, mRIDs, containers, terminal identities, limit groups, temporary limits, and individual limit records are kept. |
| Diagnose `SeriesCompensator` as unsupported | **Improved.** Series compensators now have typed electrical and identity data and emit as `SeriesCompensator`. |
| Map two winding transformers, end impedances, ratio taps, and linear phase taps | **Retained and improved.** Two and three winding transformers keep end records, complete tap step tables, ratio and phase controls, selected positions, regulated terminals, limits, and phase shifts. |
| Map the EnergyConsumer family, with SSH values and an `SvPowerFlow` fallback | **Retained and improved.** Load identity, class, containment, terminal quantities, service status, and source omissions are kept or diagnosed. |
| Map `SynchronousMachine`, its `GeneratingUnit`, reactive bounds, voltage regulation, and reference priority | **Retained and improved.** Generating unit class and identity, remote regulating terminals, reactive capability curves, targets, controls, service status, and solution quantities are kept. |
| Map `ExternalNetworkInjection` and boundary `EquivalentInjection` records | **Retained and improved.** Multi-authority boundary projection is explicit, keeps source identities and relationships where possible, and reports each unassignable value. |
| Map `LinearShuntCompensator`; diagnose nonlinear shunts | **Improved.** Linear and nonlinear shunt models, section tables, selected sections, controls, and solution sections are handled. |
| Map breaker, disconnector, load break switch, ground disconnector, jumper, fuse, and generic switch records | **Retained and improved.** Switch class, service status, normal and operating positions, retained topology, ratings, terminals, and containment are carried in detailed connectivity. |
| Map PATL, TATL, and TC limits to three branch ratings | **Retained and improved.** The balanced ratings remain available while the detailed model keeps every operational limit set, side, whole-second duration, group, and value. The source neutral model distinguishes permanent from temporary limits rather than copying every CGMES kind; PATLT and TCT substitution and fractional-duration rounding receive precise diagnostics. |
| Choose the calculation reference from the island angle reference, reference priority, external injection, or largest machine | **Retained.** Any fallback is reported. |
| Use an explicit 100 MVA base and `BaseFrequency`, with a 50 Hz fallback | **Retained.** Both assumptions use structured diagnostic codes. |
| Count unconsumed CIM classes | **Retained and tightened.** Unsupported classes, fields, records, profile data, substitutions, projections, and dropped output data use specific diagnostics rather than free text. |

## Writer

| Original behavior | PowerIO 1.0 status |
|---|---|
| Write deterministic EQ, TP, SSH, and SV sets for CGMES 2.4.15 and 3.0 | **Deliberately replaced at the public dispatcher.** PowerIO reads both releases and fresh public `emit` writes CGMES 3.0. The internal writer still exercises both encodings in tests. This avoids two public output tokens for one format. |
| Create a minimal region, substation, and voltage level hierarchy for a bus branch source | **Retained and improved.** Existing hierarchy and detailed connectivity are authoritative. Missing hierarchy is derived deterministically, and partial detailed data is completed without dropping balanced buses. |
| Write terminals and TP nodes for each balanced component | **Retained and improved.** Stable source identities are used where valid. Missing equipment, terminal, topology, and containment mRIDs use deterministic UUIDv5 values. |
| Write operating assignments in SSH and solution quantities in SV | **Retained and improved.** Loads, generation, service status, switch position, taps, shunt sections, controls, terminal powers, bus voltages, and converter quantities are written when present. Missing or conflicting data is diagnosed. |
| Write PATL, TATL, and TC current limits | **Retained and improved.** Complete limit groups and whole-second temporary durations are preserved instead of reducing every source to three branch fields. Fresh output uses PATL for permanent limits and TATL for temporary limits. |
| Encode transformer tap and phase shift into CGMES tap changer records | **Retained and improved.** Complete ratio and phase step tables, selected positions, regulating controls, transformer ends, and three winding relationships are emitted. |
| Preserve imported mRIDs and derive deterministic identifiers for new records | **Retained and tightened.** Valid imported UUID mRIDs survive. Fresh identifiers use UUIDv5 from a fixed PowerIO namespace, component type, and stable identity. Any required replacement is reported. |
| Use a fixed timestamp so repeated writes are byte stable | **Retained and improved.** An imported case date is used when present. When it is absent, the writer uses the same deterministic sentinel and reports the substitution through `EMIT.CGMES.VALUE_DEFAULTED`. |
| Warn when a non-100 MVA source cannot round trip through CGMES without rebasing | **Retained.** |
| Diagnose storage, HVDC, three winding transformers, cost curves, and remote regulation as unsupported | **Improved.** Three winding transformers, AC controls, VSC and LCC equipment, DC lines, DC switches, DC grounds, converter controls, and reactive limits are now mapped. Costs and fields with no CGMES representation remain diagnosed. |

## Tests and fixtures

| Original choice | PowerIO 1.0 status |
|---|---|
| Hand written `micro30` CGMES 3.0 profiles with value by value assertions | **Deliberately replaced.** Small synthetic unit cases cover the same electrical equations and a wider set of records without carrying a second fixture tree. |
| CIGRE MV and `sample_grid_switches` copied from cimpy at `ac400d43c015afc4ac58e7e833b91fcba6f32812`, Apache-2.0 | **Deliberately replaced.** They were license clean, but the final tests use generated cases and a pinned PowSybl Core checkout with broader CGMES coverage. No behavior depended on deleting them. |
| Do not vendor ENTSO-E conformity files because their terms are not an SPDX license | **Retained.** The gate reads the copies distributed in the pinned PowSybl Core checkout and does not copy them into this repository. |
| Parse checks for electrical values, switching topology, warnings, and both CGMES releases | **Improved.** Unit tests cover both releases, source shapes, security limits, profile requirements, mappings, diagnostics, deterministic output, and failure cases. |
| Writer read back and byte stability tests | **Retained and improved.** Unit tests exercise both internal encodings. The external gate removes retained source bytes, writes fresh CGMES, loads it with PowSybl, and compares identities, terminals, hierarchy, calculated buses, switches, connectivity, controls, limits, operating assignments, and solution quantities. Raw XML checks also pin the source and fresh operational limit kinds. |
| Documentation of the CIM and CGMES version map, profile roles, assumptions, supported records, and planned work | **Retained and updated.** Current format documentation states the exact 1.0 input and output coverage. This file records what happened to the original implementation. |

## Deferred work

These items remain unsupported and are reported rather than silently removed:

- topology calculation from an EQ file with no TP profile;
- CGMES DifferenceModel input;
- complete multi authority CGM assembly and lossless EQBD, TPBD, and TieFlow
  output;
- typed or fresh DL, GL, and DY profile output;
- distribution CIM and CDPSM; and
- RDFS or SHACL validation against the Application Profiles Library.

Fresh output contains EQ, TP, SSH, and SV. One source neutral shunt record also
has one selected section count, so differing SSH and SV section counts cannot
both be represented after retained source bytes are removed; the reader and
writer diagnose that reduction.
