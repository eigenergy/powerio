# PowSybl IIDM serialization fixtures

Every file under `V1_<minor>/` is copied unchanged from the PowSybl Core
repository (`https://github.com/powsybl/powsybl-core`, commit
`a795bfac3c`), directory
`iidm/iidm-serde/src/test/resources/V1_<minor>/`. PowSybl Core is licensed
under the Mozilla Public License 2.0 (`MPL-2.0`); the files carry no
ENTSO-E data.

| File | Versions | Purpose |
| --- | --- | --- |
| `eurostag-tutorial1-lf.xml` | 1.0 to 1.17 | The same solved four bus network written by PowSybl in every XIIDM version: two substations, a generator, a load, two lines, and two transformers with a voltage regulating ratio tap changer. |
| `eurostag-tutorial1-lf.json` | 1.11 to 1.17 | The same network as PowSybl JIIDM JSON. |
| `fictitiousSwitchRef.xml`, `fictitiousSwitchRef.jiidm` | 1.0, 1.11, 1.17 | A node breaker network with busbar sections, switches, calculated buses, a phase tap changer, and operational limits. IIDM 1.0 states the bus voltages on the busbar sections. |
| Battery reference | 1.0 | Batteries with the `p0`/`q0` attribute names IIDM used before 1.8. |
| Nonlinear shunt reference | 1.0 | A shunt with `bPerSection`, `maximumSectionCount`, and `currentSectionCount` stated on the element, as every version before 1.3 does. |
| Static VAR compensator reference | 1.0 | A static VAR compensator with the `voltageSetPoint` spelling IIDM used before 1.3. |
| Three winding transformer reference | 1.0 | A three winding transformer without `ratedU0`, with leg 2 and 3 ratio tap changers regulating `targetV`, and with current limits stated directly on the element. |
| `tl-loading-limits.xml` | 1.0 | Tie lines in the inline form IIDM used before 1.10, with current limits per side. |

No file was edited. Each file is smaller than 20 KiB.
