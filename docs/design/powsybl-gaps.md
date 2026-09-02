# powerio versus PowSybl Core: reader and writer gap list

Scope: every grid exchange format both projects implement. That is MATPOWER, PSS/E RAW and RAWX, XIIDM, and CGMES. PowSybl's AMPL, UCTE, IEEE CDF, and PowerFactory converters have no powerio counterpart and are not compared. PowSybl's `.mat` binary MATPOWER container is compared against powerio's `.m` text reader and writer because both carry the same `mpc` tables.

Repositories read:

- powerio at `/home/sam/Research/powerio`, branch `agent/cli-composability`.
- PowSybl Core at `/home/sam/Research/powsybl-core`, commit `a795bfac3c` (2026-08-31). The interoperability gate in `evals/powsybl/` pins PowSybl Core 7.3.0 at `0939bfcc2c0c094de907dc818dd688b4cbfb7281`; line numbers below are from the checkout present on disk.

Nothing was run. Every file:line was read.

## How to read the tables

Columns: element or attribute; what PowSybl does (file:line); what powerio does (file:line); gap kind; fix size.

Gap kinds:

- `powerio drops it`: the data is lost with no diagnostic.
- `powerio defaults it`: powerio substitutes a value where the source states one, or invents one where PowSybl keeps the absence.
- `powerio warns where PowSybl carries it`: powerio reports the loss and PowSybl represents the data.
- `PowSybl warns too`: both projects lose it and PowSybl logs or reports the loss.
- `powerio does more`: powerio carries something PowSybl drops or does not model.
- `no gap`: both carry it; listed only where the task required the row.

Fix sizes: `small` is one match arm or one branch in one function; `medium` is one struct field or one option threaded through a reader and a writer; `large` is a new model concept in `network.rs` with reader, writer, IR, and validation work.

File aliases used throughout (absolute paths):

| alias | path |
|---|---|
| net | /home/sam/Research/powerio/powerio-tx/src/network.rs |
| norm | /home/sam/Research/powerio/powerio-tx/src/normalize.rs |
| fmt | /home/sam/Research/powerio/powerio-tx/src/format/mod.rs |
| diag | /home/sam/Research/powerio/powerio-tx/src/diagnostics.rs |
| err | /home/sam/Research/powerio/powerio-tx/src/error.rs |
| mp-mod, mp-rows, mp-wr, mp-loc, mp-ml | /home/sam/Research/powerio/powerio-tx/src/format/matpower/{mod,rows,writer,locate,matlab}.rs |
| psse | /home/sam/Research/powerio/powerio-tx/src/format/psse.rs |
| rawx | /home/sam/Research/powerio/powerio-tx/src/format/rawx.rs |
| xi | /home/sam/Research/powerio/powerio-tx/src/format/xiidm.rs |
| cg-mod, cg-read, cg-write | /home/sam/Research/powerio/powerio-tx/src/format/cgmes/{mod,read,write}.rs |
| PI | /home/sam/Research/powsybl-core/matpower/matpower-converter/src/main/java/com/powsybl/matpower/converter/MatpowerImporter.java |
| PE | /home/sam/Research/powsybl-core/matpower/matpower-converter/src/main/java/com/powsybl/matpower/converter/MatpowerExporter.java |
| PR, PW | /home/sam/Research/powsybl-core/matpower/matpower-model/src/main/java/com/powsybl/matpower/model/{MatpowerReader,MatpowerWriter}.java |
| MB, MG, MBR, MD, MFV | same directory as PR: MBus.java, MGen.java, MBranch.java, MDcLine.java, MatpowerFormatVersion.java |
| ST | /home/sam/Research/powsybl-core/iidm/iidm-extensions/src/main/java/com/powsybl/iidm/network/extensions/SlackTerminal.java |
| XD | /home/sam/Research/powsybl-core/docs/grid_exchange_formats/matpower/export.md |

Aliases for the PSS/E, XIIDM, and CGMES sources are declared at the head of each section.

Two facts about powerio shape many rows:

1. The neutral model is `BalancedNetwork` (net:1460-1539): `Bus` with a MATPOWER `BusType` (net:1187-1192), `Load` with an optional `LoadVoltageModel` (net:2103-2135), `Shunt` with an optional switched shunt control (net:2168-2265), `Branch` with `rate_a/b/c`, `rating_sets`, `current_ratings`, `tap`, `shift`, and an optional `TransformerControl` (net:2340-2393, 2770-2793), `Generator` with `regulated_bus`, `regulating_terminal`, `voltage_regulation_on`, and `active_power_control` (net:2866-2919), `Hvdc` with the 17 MATPOWER dcline columns plus optional converter records (net:3133-3169), `Area` (net:3234-3251), `Transformer3W` (net:3387-3405), and an optional `DetailedConnectivity` (net:1079-1137) that holds substations, voltage levels, nodes, terminals, switches, tap changers, operational limit groups, reactive limits, and DC equipment. Anything not in that model is lost between formats unless a writer reads it from `extras` or `DetailedConnectivity`.
2. Slack designation happens in `normalize.rs`, not in readers or writers: a bus hosting an in-service generator keeps `Ref` when the file says so, otherwise becomes `Pv`; a generator-less bus becomes `Pq`; when no `Ref` survives, the bus of the largest-`pmax` in-service generator is promoted and `CANONICALIZE.NORMALIZE.REFERENCE_DESIGNATED` is reported (norm:819-833, 635-662). This is one global choice, not one per synchronous component.

---

## 1. MATPOWER

Structural facts:

- PowSybl reads and writes only the `.mat` v5 binary struct. Its reader touches `version`, `baseMVA`, `bus`, `gen`, `branch`, and the optional `bus_name` and `dcline` (PR:56-77). `gencost`, `dclinecost`, `storage`, `areas`, `gentype`, and `genfuel` never enter its model. powerio reads and writes `.m` text and carries all of those.
- powerio's MATPOWER reader has no diagnostics channel: `read_source` calls `matpower::parse_matpower_source(text, name_hint)` with no warnings argument (fmt:797), unlike every other reader (fmt:798-825). Every reader-side drop below is therefore silent today. Adding any reader warning is a medium change: thread `&mut Diagnostics` through `parse_matpower_source`, `parse_matpower_named`, and `build_case`.

### 1.1 MATPOWER import (file to model)

| element or attribute | PowSybl (file:line) | powerio (file:line) | gap kind | fix size |
|---|---|---|---|---|
| file container | `.mat` v5 binary only, struct `mpc` (PR:51-55) | `.m` text, assignments located by scan (mp-mod:22-31, mp-loc:32-72) | powerio does more | none |
| `mpc.version` | required, must be "2", else exception (PR:56-64, MFV:31-37) | never read (mp-mod:82-150); `leak_field` lists it but nothing calls it (mp-ml:192) | powerio does more | none |
| required blocks | version, baseMVA, bus, gen, branch (PR:56-60) | baseMVA, bus, branch (mp-mod:83-88, 105-106); gen optional (mp-mod:226-229) | powerio does more | none |
| `baseMVA` | model base, used for the impedance base (PR:66; PI:292, 349, 378) | verbatim (mp-mod:83-86) | no gap | none |
| base frequency | no concept | `DEFAULT_BASE_FREQUENCY` 60 Hz (mp-mod:132; net:1143) | powerio defaults it | none |
| column count checks | bus >= 13, gen >= 10, branch >= 13, dcline >= 17 else exception; gen with 10..20 columns read as version 1 with `LOGGER.warn` (PR:101-123) | per-row `ShortRow` error: bus 13 (mp-rows:36, 155), branch 13 (mp-rows:54, 201), gen 10 (mp-rows:91, 234), dcline 17 (mp-rows:76, 330), storage 17 (mp-rows:113, 297), gencost 4+ncost (mp-rows:123, 265-286), areas 2 (mp-rows:365); a gen row with 10..20 columns fills only the present cap slots, no warning (mp-rows:238-241) | powerio defaults it | medium (needs the reader warnings channel) |
| BUS_I | `getInt` column 0, id `BUS-<n>` (PR:128; PI:104-106, 233) | `id_from_f64` refuses negative, non-finite, over 2^63 (mp-rows:142-148; fmt:858-872); duplicates and dangling references rejected by `check_references` (mp-mod:73; net:4000-4099) | powerio does more | none |
| BUS_TYPE codes outside 1..4 | `MBus.Type.fromInt` throws (MB:39-44) | `from_f64` maps anything not 2/3/4 to PQ silently (net:1196-1203; mp-rows:157) | powerio defaults it | small (one match arm: error or warning) |
| bus type 1 (PQ), 2 (PV) | no effect on IIDM; only REF is used (PI:126-128) | kind kept verbatim (mp-rows:157) | powerio does more | none |
| bus type 3 (REF) and slack selection | every type 3 bus gets `SlackTerminal.attach` on its voltage level (PI:126-128, 621-623); attach throws when the bus has no connected terminal (ST:62-71); two REF buses merged into one voltage level: the second `reset` replaces the first (ST:70) | kind Ref kept verbatim, several allowed (mp-rows:157); normalize keeps every Ref hosting an in-service generator, demotes generator-less Ref to PQ, designates the largest-pmax generator bus only when none survives (norm:819-833, 632-662) | powerio does more | none |
| bus type 4 (ISOLATED) | created as an ordinary bus; loads, shunts, generators attached normally (PI:232-242, 273-307) | kind Isolated; its Load and Shunt get `in_service = false` (mp-rows:158, 182, 191); normalize drops the bus and everything on it (norm:781-783) | powerio does more | none |
| PD/QD to Load | `LOAD-<n>` only when Pd != 0 or Qd != 0 (PI:274-285) | Load only when nonzero (mp-rows:176-185) | no gap | none |
| GS/BS to Shunt | shunt only when Bs != 0; a Gs-only shunt is dropped silently (PI:289); linear model, 1 section, g/b per section = Gs/baseMVA/zb (PI:293-303), `voltageRegulatorOn` false | Shunt when gs != 0 or bs != 0, g/b in MW/MVAr at 1 pu (mp-rows:186-196) | powerio does more | none |
| BUS_AREA | read into `MBus` (PR:138), never used; no IIDM Area created | `bus.area` (mp-rows:169) | powerio does more | none |
| ZONE | read into `lossZone` (PR:142), never used | `bus.zone` (mp-rows:170) | powerio does more | none |
| VM/VA | `setV(vm * nominalV)`, `setAngle(va)` (PI:238-239) | verbatim (mp-rows:162-163) | no gap | none |
| BASE_KV | parameter `matpower.import.ignore-base-voltage` defaults true, so nominalV = 1 for every voltage level unless disabled; base 0 also gives 1 (PI:57-60, 255-257, 597-598) | verbatim, 0 kept (mp-rows:164); `zbase()` treats 0 as 1 (fmt:1755-1761) | powerio does more | none |
| VMAX/VMIN | per voltage level, most restrictive over member buses, times nominalV; 0 means unset; inverted band: `LOGGER.warn` and swap (PI:147-189) | per bus verbatim, no check at parse (mp-rows:165-166) | powerio does more | none |
| `mpc.bus_name` | cell read per row when present (PR:71-73, 129-132) | attached only when the cell count equals the bus count, otherwise every name is dropped with no diagnostic (mp-mod:119-127); empty string reads as unnamed (mp-mod:124) | powerio drops it | medium (warning needs the reader channel) |
| bus columns 14-17 (LAM_P, LAM_Q, MU_VMAX, MU_VMIN) | ignored (PR:128-144) | ignored (mp-rows:155-175) | no gap | none |
| GEN_BUS, generator id | `GEN-<bus>`, `ensureIdUnicity` (PI:193-197) | `Generator.bus`, uid None (mp-rows:243, 260) | no gap | none |
| PG/QG | targetP/targetQ (PI:201-202) | pg/qg (mp-rows:245-246) | no gap | none |
| QMAX/QMIN | `MinMaxReactiveLimits` unless Pc1 or Pc2 nonzero (PI:222-227) | qmax/qmin (mp-rows:247-248) | no gap | none |
| VG voltage setpoint and regulation | targetV = Vg * nominalV; `voltageRegulatorOn = (Vg != 0)` (PI:200, 203) | vg verbatim (mp-rows:249); `voltage_regulation_on` always true (mp-rows:256) | powerio defaults it | small (`voltage_regulation_on: row[VG] != 0.0`) |
| remote regulation | not expressible in MATPOWER; regulating terminal is the generator's own | `regulated_bus` None, `regulating_terminal` None (mp-rows:257-258) | no gap | none |
| MBASE | ratedS = mBase, 0 gives NaN (PI:206) | mbase verbatim (mp-rows:250) | no gap | none |
| GEN_STATUS | in service iff status > 0 (PI:317-319); out-of-service generators keep a connectable bus (PI:198-199) | in service iff status == 1.0 exactly (mp-rows:14-19, 253); status 2 reads as out of service | powerio defaults it | small (`status > 0.0`) |
| PMAX/PMIN | maxP/minP (PI:204-205) | pmax/pmin (mp-rows:251-252) | no gap | none |
| PC1, PC2, QC1MIN, QC1MAX, QC2MIN, QC2MAX | when Pc1 != 0 or Pc2 != 0: two-point `ReactiveCapabilityCurve` (PI:209-221); read only when gen has 21 columns (PR:163-175) | `GenCaps` slots 0-5 verbatim (mp-rows:238-241; net:3503-3506); no curve concept in the balanced model | no gap | none |
| RAMP_AGC, RAMP_10, RAMP_30, RAMP_Q, APF | read into `MGen` (PR:170-174), never mapped to IIDM | `GenCaps` slots 6-10 (mp-rows:238-241) | powerio does more | none |
| gen columns past 21 | ignored (PR:150-178) | ignored (mp-rows:239) | no gap | none |
| `mpc.gencost` active block | not read (PR:56-77) | first n rows attached in order; count must be n or 2n else `GenCostCountMismatch` (mp-mod:234-250); model/startup/shutdown/ncost/coeffs, mixed-width padding trimmed, oversized NCOST rejected (mp-rows:264-294) | powerio does more | none |
| `mpc.gencost` reactive block (rows n+1..2n) | not read | accepted, then discarded silently (mp-mod:245-249) | powerio drops it | medium (new `Generator.reactive_cost: Option<GenCost>` threaded through reader and writer) |
| gencost piecewise (model 1), startup, shutdown | not read | kept (mp-rows:280-284, 289-290) | powerio does more | none |
| F_BUS/T_BUS, branch id | `LINE-<f>-<t>` or `TWT-<f>-<t>`, `ensureIdUnicity` (PI:388-389, 420-421) | `Branch.from/to`, name None, uid None (mp-rows:203-205, 222) | no gap | none |
| BR_R, BR_X, BR_B | line: to ohms and siemens allowing different nominal voltages at the ends (PI:376-385, 458-469); r = x = 0 gives zero admittance (PI:453-456); transformer: r*zb, x*zb, b/zb on the side 2 nominal voltage (PI:349, 430-433) | per unit verbatim, `charging` None (mp-rows:206-209) | no gap | none |
| RATE_A/B/C | only when RateA != 0: `ApparentPowerLimits` on both sides, permanent = RateA, temporary "RateB" 1200 s when != 0, "RateC" 60 s when != 0 (PI:321-338, 358-365); RateB/RateC with RateA == 0 are dropped | rate_a/b/c verbatim (mp-rows:210-212) | powerio does more | none |
| TAP (ratio) | line iff shift == 0 and (ratio == 0 or (ratio == 1 and equal base kV)) (PI:88-98); transformer: ratedU1 = nominalV1 * ratio (0 treated as 1), ratedU2 = nominalV2 (PI:416, 428-429) | tap verbatim, 0 means line (mp-rows:215; net:2368-2369, 2651-2653) | no gap | none |
| SHIFT | shift != 0: one-step `PhaseTapChanger`, rho 1, alpha = -shift (PI:435-447) | shift verbatim (mp-rows:216) | no gap | none |
| tap changer tables | none in MATPOWER; one synthetic step | `Branch.control` None (mp-rows:220); `DetailedConnectivity` None (mp-mod:135) | no gap | none |
| BR_STATUS | in service iff abs(status) > 0 (PI:309-311); status -1 is in service | in service iff status == 1.0 (mp-rows:217) | powerio defaults it | small |
| ANGMIN/ANGMAX | read into `MBranch` (PR:195-196), never used | angmin/angmax verbatim (mp-rows:218-219) | powerio does more | none |
| branch columns 14-17 (PF, QF, PT, QT) and 18-21 (MU_*) | ignored (PR:181-198) | ignored; `Branch.solution` stays None (mp-rows:221) | no gap | small (optional: fill `solution` from columns 14-17 when present) |
| `mpc.dcline` | two `VscConverterStation` `CS1-/CS2-<f>-<t>`, `voltageRegulatorOn` true, setpoints Vf*Vnom1 and Vt*Vnom2, lossFactor1 = loss0*100/Pf, lossFactor2 from the remaining loss (PI:483-501, 522-533); `HvdcLine` `HL-<f>-<t>`, R = 0, SIDE_1_RECTIFIER_SIDE_2_INVERTER, activePowerSetpoint = Pf, nominalV = Vnom1, maxP = Pmax (PI:502-511); Qminf/Qmaxf and Qmint/Qmaxt only when finite and min <= max (PI:513-518, 535-537); Pt, Qf, Qt, Pmin never used; status gates connection (PI:480-482) | all 17 columns verbatim into `Hvdc` (mp-rows:329-358); converter1/2 and converters_mode None | powerio does more | none |
| `mpc.dclinecost` | not read | one row per dcline required; an all-zero row reads as no cost (mp-mod:193-222) | powerio does more | none |
| `mpc.storage` | not read | 17-column table (mp-rows:296-327) | powerio does more | none |
| `mpc.areas` | not read | `[area, refbus]`, refbus 0 reads None (mp-rows:360-371) | powerio does more | none |
| `mpc.gentype`, `mpc.genfuel` | not read | `parse_string_cell` is documented for them (mp-loc:74-78) but `build_case` never reads them; `energy_source` stays Other (mp-rows:244) | powerio drops it | small (reader: map genfuel to `GeneratorEnergySource`) plus small (writer: emit `mpc.genfuel`) |
| case name | network id and caseName from the data source base name; caseDate = now (PI:586-594) | file stem, else `function mpc = <name>`, else "case" (mp-mod:22-55) | no gap | none |
| substations and voltage levels | synthesized from transformer and zero-impedance adjacency, ids `SUB-/VL-<min bus>` (PI:602-611) | none; `detailed_connectivity` None (mp-mod:135) | no gap | none |
| reference integrity (unknown bus in branch, gen, load) | no explicit check; `getVoltageLevel` returns null and later calls throw (PI:347-349) | `check_references`, `PARSE.SOURCE.MALFORMED` (mp-mod:73; net:4000-4099) | powerio does more | none |
| empty case | no check | `reject_empty_case` "case has no buses or DC equipment" (fmt:828, 877-896) | powerio does more | none |
| Inf and NaN tokens | binary doubles | `parse_float` accepts Inf, -Inf, NaN spellings (mp-ml:169-178) | no gap | none |
| reader warnings channel | `LOGGER.warn` sites at PR:110 and PI:158 | none; the reader returns only errors (fmt:797) | powerio drops it | medium (prerequisite for every reader warning above) |

### 1.2 MATPOWER export (model to file)

| element or attribute | PowSybl (file:line) | powerio (file:line) | gap kind | fix size |
|---|---|---|---|---|
| file container and blocks | `.mat` v5, always version "2", baseMVA, bus, gen (21 columns), branch (13), optional dcline, optional bus_name (PW:32-53, 98-99, 177-188) | `.m` text: function header, version '2', baseMVA, bus, bus_name, branch, areas, gen (10 or 21), gencost, dcline, dclinecost, storage (mp-wr:237-462) | powerio does more | none |
| same format pass-through | none | an unchanged parsed MATPOWER module writes the retained text with no diagnostics (fmt:1045-1047, 1062-1081) | powerio does more | none |
| `baseMVA` | fixed 100; everything rebased on it (PE:45, 1198) | `net.base_mva()` verbatim (mp-wr:240) | powerio does more | none |
| which buses are written | bus view buses in the main synchronous component plus components reached through exported VSC dclines (PE:150-215, 321-323; XD:4-5); all other buses dropped silently; plus star buses and dangling line buses (PE:257-283, 293-319) | every bus in the model, including Isolated and disconnected islands (mp-wr:243-263) | powerio does more | none |
| bus numbering | preserve n from ids `BUS-<n>` (min over merged bus breaker buses), else sequential after the largest preserved (PE:217-255); star and dangling line buses numbered by equipment id (PE:264, 300) | `BusId` verbatim (mp-wr:249) | no gap | none |
| BUS_TYPE derivation | REF when `SlackTerminal` matches the bus or auto-selected; else PQ, promoted to PV when a connected regulating generator exists (PE:106-122, 1087-1089); star bus ISOLATED only when all three legs are disconnected (PE:285-291) | `kind as u8` verbatim, no derivation from generators (mp-wr:250); derivation lives only in normalize (norm:819-833) | no gap | none |
| slack selection | one REF per exported synchronous component: `SlackTerminal` if any, else max sum of generator maxP, then count of regulating VSCs, then bus id (PE:1138-1162) | writes Ref as stated; when none, `EMIT.MATPOWER.REFERENCE_MISSING` (fmt:1615-1641); normalize designates one global slack, not one per island (norm:632-662, 831-833) | powerio warns where PowSybl carries it | medium (per-component designation in the writer or as a normalize option) |
| multiple REF buses | one per synchronous component; several `SlackTerminal` give several REF | written as stated | no gap | none |
| isolated buses | not exported (not in the bus view) except the star bus case above | type 4 written verbatim (mp-wr:250) | powerio does more | none |
| PD/QD | sum of connected loads P0/Q0, minus battery targetP/targetQ, plus LCC converter P and Q (PE:332-348); minus connected regulation-off generators at PV buses (PE:1092-1096); star bus 0, dangling line bus P0/Q0 (PE:272-273, 308-309) | sum of in-service loads only (mp-wr:224-229, 244); out-of-service loads dropped with `RECORD_DROPPED` (mp-wr:127-142) | powerio does more | none |
| load voltage model (ZIP, exponential) | only P0/Q0 read; IIDM load models ignored silently (PE:334-337) | `FIELD_DROPPED` when `has_non_matpower_fields`; totals still written (mp-wr:161-174; net:2139-2162) | powerio does more | none |
| GS/BS | sum over connected shunt compensators of getG/getB at the current section times Vnom^2 (PE:349-357); other sections and regulation lost silently | sum of in-service shunts g/b (mp-wr:230-235, 245); out-of-service shunts dropped with `RECORD_DROPPED` (mp-wr:143-149); `Shunt.section_count` and `Shunt.control` (switched shunt blocks, band, control bus; net:2176-2184, 2252-2265) dropped with no warning | powerio drops it | small (one warning branch in `canonical_warnings`) |
| VM/VA | V/Vnom, NaN or <= 0 gives 1.0; angle NaN gives 0 (PE:358-359, 1130-1136) | verbatim; NaN written as the NaN token (mp-wr:257-258) | no gap | none |
| VMAX/VMIN | voltage level high/low limit over Vnom, NaN gives 0 (PE:360-361, 370-372) | verbatim (mp-wr:260-261); evhi/evlo dropped with `FIELD_DROPPED` (mp-wr:73-82) | no gap | none |
| BUS_AREA, ZONE | constants 1 and 1 (PE:46-47, 267-268, 329-330) | `bus.area`, `bus.zone` verbatim (mp-wr:255, 259) | powerio does more | none |
| `mpc.areas` | never written | `[number, refbus or 0]` (mp-wr:301-309); name, interchange, tolerance, uid, area_type dropped with `FIELD_DROPPED` (mp-wr:191-208) | powerio does more | none |
| `mpc.bus_name` | only with parameter `matpower.export.with-bus-names` (default false), name = `getNameOrId` (PE:54-65, 1193, 1225; PW:77-96) | written when any bus is named; quote doubling, control characters to space, empty for unnamed (mp-wr:33-44, 266-277) | powerio does more | none |
| branch set | lines, tie lines, 2W transformers, dangling lines, 3W legs (PE:798-804); only when both buses are exported (PE:659); both ends on one bus skipped with `LOGGER.warn` (PE:676) | every `Branch` (mp-wr:279-299) | no gap | none |
| BR_STATUS | 1 iff both terminals connected (PE:664, 986-988) | `f64::from(in_service)` (mp-wr:294) | no gap | none |
| BR_R, BR_X | per unit on 100 MVA and the nominal voltages of both ends (PE:647-648, 687-691; transformer on the side 2 voltage level, PE:618-620); hypot(r, x) < 1e-8 cut to r = 0, x = 1e-8 with `LOGGER.warn` (PE:52, 785-796) | verbatim, no low-impedance check in the writer (mp-wr:287-288); matrix builders reject later with `BUILD.BRANCH.ZERO_IMPEDANCE` (err:44-45) | no gap | small (optional `VALUE_SUBSTITUTED` cut) |
| BR_B | b1pu + b2pu with the different-nominal-voltage correction (PE:650-653, 693-699); line g1/g2 dropped silently (`createBranch` takes only b1, b2, PE:643) | total of terminal charging (mp-wr:289; net:2559-2562); conductance or asymmetric charging collapsed with `VALUE_COLLAPSED` (mp-wr:83-92) | powerio does more | none |
| RATE_A | permanent limit of the chosen limit set: apparent power limits first, then current limits, from either side, needing a permanent value, preferring the set with the most temporary limits (PE:397-401, 416-428) | rate_a verbatim (mp-wr:290) | no gap | none |
| RATE_B | longest-duration temporary limit with duration > 60 s and value != MAX_VALUE (PE:374-381, 402-408) | rate_b verbatim (mp-wr:291) | no gap | none |
| RATE_C | the limit whose duration is just above the shortest emergency limit (<= 60 s), when not the same as RATE_B (PE:383-391, 409-413) | rate_c verbatim (mp-wr:292) | no gap | none |
| current limits to MVA | amps converted by I * Vnom / 1000 using the side 1 voltage level (PE:393-395, 422-423; PowSybl omits the sqrt(3) factor) | `Branch.current_ratings` dropped with `FIELD_DROPPED` even when rate_a/b/c are 0 (mp-wr:93-102; net:2470-2474) | powerio warns where PowSybl carries it | small (`VALUE_SUBSTITUTED`: fill a zero rate_* from current ratings times base_kv times sqrt(3)/1000) |
| extra rating sets | none | `RATING_SET_DROPPED` per set (mp-wr:150; fmt:1584-1598) | powerio does more | none |
| active power limits | ignored (PE:417-418) | no such field on `Branch` | no gap | none |
| TAP and SHIFT | rho = (ratedU2/Vnom2)/(ratedU1/Vnom1) times the ratio and phase current-step rho; r, x, b scaled by the step percent deviations; ratio = 1/rho; shift = -alpha (PE:597-617); lines 0/0 (PE:653) | tap and shift verbatim (mp-wr:293-294); the XIIDM reader folds steps the same way into `Branch.tap/shift` (xi:4476-4512) | no gap | none |
| tap changer tables (steps, regulation) | collapsed to the current step, silently | `DetailedConnectivity.tap_changers` (net:1111) and `Branch.control` (net:2770-2793) not consulted and not warned | powerio drops it | small (one warning branch for `control.is_some()`) |
| ANGMIN/ANGMAX | never set; `MBranch` 0/0 written (MBR:76-81; PW:142-143), which MATPOWER reads as unconstrained | verbatim (mp-wr:295-296) | powerio does more | none |
| three winding transformers | star bus per transformer (base kV = ratedU0, vm from the "v" property or 1, type PQ or ISOLATED) plus one branch per leg with the leg impedance on the ratedU0 base and the leg tap changer folded (PE:257-283, 722-783) | `RECORD_DROPPED` "star-expand them into branches before writing" (mp-wr:63-72); `expand_transformers_3w` exists but is crate-private and not called by the writer (net:3921; `to_star_expansion` net:3449-3498) | powerio warns where PowSybl carries it | medium (call `expand_transformers_3w` in `write_matpower`, star id = max bus id + 1 + k, drop the warning) |
| switches | the bus view merges closed retained switches and splits on open ones; no rows written (PE:322, 657-658, 965-967) | `RECORD_DROPPED` "MATPOWER has no switch table" (mp-wr:54-62); endpoints stay separate buses with no connecting branch | powerio warns where PowSybl carries it | medium (write a closed switch as a branch with the writer's minimum impedance, or merge the endpoints) |
| generator set | generators, SVCs, dangling line generation, VSC converters not exported as dcline (PE:806-882, 1039-1058); rules PE:1066-1099: connected regulating gives gen; disconnected regulating gives out-of-service gen; connected non-regulating at a bus with no regulating gen gives gen with Vg = 0; at a PV bus folded into Pd/Qd; disconnected non-regulating discarded | every `Generator` row (mp-wr:311-340); `static_var_compensators` never written and never warned (no check in mp-wr:49-210) | powerio drops it | small (warning) or medium (write an SVC as a gen row: pmax = pmin = 0, qmin/qmax = b_min/b_max times Vnom^2, vg = setpoint/Vnom) |
| PG/QG | targetP; targetQ NaN gives 0 (PE:1105-1106) | verbatim (mp-wr:323-324) | no gap | none |
| VG | targetV over the regulating terminal Vnom; NaN or <= 0 gives 1.0 (PE:855-857, 1007-1009); regulation off: Vg = 0 or folded to load (PE:1080-1085, 1092-1096) | vg verbatim (mp-wr:327); `voltage_regulation_on == false` ignored, no warning | powerio drops it | small (write vg = 0 when regulation is off, one `VALUE_SUBSTITUTED` warning) |
| remote regulation | localized with `LOGGER.warn` "Generator remote voltage control not supported in Matpower model, control has been localized" (PE:854-857, 1116-1118, 1126-1128) | `regulated_bus` and `regulating_terminal` (net:2904-2912) dropped with no warning | PowSybl warns too | small (one `FIELD_DROPPED` warning when `regulated_bus` differs from `bus`) |
| PMAX/PMIN/QMAX/QMIN | clamped to parameters `max-generator-active/reactive-power-limit` (default 10000) (PE:55-60, 1109-1112); Q limits taken from the reactive limits at targetP, swapped when inverted (PE:842-849) | verbatim, Inf written as Inf (mp-wr:325-326, 330-331) | no gap | none |
| MBASE | ratedS, NaN gives 0 (PE:1113) | verbatim (mp-wr:328) | no gap | none |
| GEN_STATUS | terminal connected, then the rules above (PE:982-984, 1075-1077) | `f64::from(in_service)` (mp-wr:329) | no gap | none |
| PC1..APF columns | always 21 columns, all zero (MGen defaults; PW:99, 112-122); a reactive capability curve is collapsed to min/max at targetP | 21 columns only when any generator has caps, zero padding with `VALUE_DEFAULTED` (mp-wr:103-122, 316-339) | powerio does more | none |
| `mpc.gencost` | never written | all-or-nothing block; a partial set is dropped with `FIELD_DROPPED` (mp-wr:175-185, 342-367); rectangular padding for mixed models | powerio does more | none |
| gencost reactive block | never written | never written (discarded at read, mp-mod:245-249) | no gap | see the import row |
| energy source, active power control | no concept | `energy_source`, `active_power_control` dropped silently | no gap | small (with the genfuel writer row above) |
| storage | batteries folded into Pd/Qd with generator sign (PE:338-342) | `mpc.storage` 17 columns (mp-wr:434-460) | powerio does more | none |
| `mpc.dcline` (VSC) | only VSC to VSC with voltage regulation on at both ends (PE:889-917); rectifier side by convertersMode (PE:897-901); Pmin 0, Pmax maxP, Pf = -rectifier targetP, Pt = inverter targetP, Qf/Qt reactive setpoints (NaN gives 0), Vf/Vt setpoint over Vnom, Q limits at targetP clamped to +/-Integer.MAX_VALUE, loss0 = lossFactor*Pf/100, loss1 = (losses - loss0)/Pf (PE:919-963, 1003-1037) | every `Hvdc` row, 17 columns verbatim (mp-wr:370-395); the XIIDM reader derives pt, loss0, loss1 from the converters (xi:4997-5063) | no gap | none |
| `mpc.dcline` (LCC) | LCC converters folded into Pd/Qd as load (PE:343-346); no dcline row | written as a dcline row regardless of `converter.kind` | powerio does more | none |
| VSC with regulation off | one regulating converter becomes a generator, the other a load; both off: both loads (PE:906-917, 1039-1058; XD:39) | dcline row regardless of `voltage_regulator_on` | powerio does more | none |
| converter station metadata | consumed (loss factor, setpoints, reactive limits) | `converter1`, `converter2`, `converters_mode`, `resistance_ohm`, `nominal_voltage_kv` (net:3151-3162) dropped with no warning | powerio drops it | small (one `FIELD_DROPPED` warning) |
| `mpc.dclinecost` | never written | written when any line has a cost, zero rows for the rest (mp-wr:397-431) | powerio does more | none |
| base frequency | no concept | `FIELD_DROPPED` when != 60 Hz (fmt:1433-1452) | powerio does more | none |
| bus locations, branch routes | no concept | `FIELD_DROPPED` (fmt:1460-1480) | powerio does more | none |
| extras (passthrough fields) | IIDM properties and extensions ignored silently | `EXTRAS_DROPPED` count (mp-wr:186-190; fmt:1534-1563) | powerio does more | none |
| branch solution PF/QF/PT/QT | 13 columns only | `FIELD_DROPPED` (mp-wr:151-160) | powerio does more | small (optional: write columns 14-17 like MATPOWER's own `savecase`) |
| element ids, names, uids | lost; "each exported generator ... identified only by its bus" (XD:29) | `Branch.name` and every uid dropped silently | no gap | none |
| case name | caseName = network id but the struct is always `mpc`; name not written (PE:1197; PW:183) | `function mpc = <ident>`, sanitized to a legal identifier (mp-wr:238, 465-483) | powerio does more | none |
| dangling lines and tie lines | extra bus plus branch plus optional generator; tie lines as branches (PE:293-319, 630-641, 701-720, 806-828) | the writer reads only the balanced tables; whatever the XIIDM reader lowered into Bus/Branch is written; the `DetailedConnectivity` dangling line and tie line lists (net:1115-1117) not consulted | no gap | none |
| NaN and Inf values | replaced by 1.0, 0, or Integer.MAX_VALUE (PE:1003-1019, 1130-1136) | written as tokens; the family's `not_a_number` entry is never used by this writer | no gap | none |

### 1.3 powerio MATPOWER diagnostic emission sites

Writer, `canonical_warnings` (mp-wr:49-210), family `EMIT_MATPOWER` (diag:172), aliased `F` at mp-wr:15.

| code | message literal | condition | file:line |
|---|---|---|---|
| EMIT.MATPOWER.RECORD_DROPPED | `"{n} switch(es) dropped: MATPOWER has no switch table"` | `!net.switches().is_empty()` | mp-wr:54-62 |
| EMIT.MATPOWER.RECORD_DROPPED | `"{n} 3-winding transformer(s) dropped: the canonical MATPOWER writer emits no 3-winding record (star-expand them into branches before writing to keep them)"` | `!net.transformers_3w().is_empty()` | mp-wr:63-72 |
| EMIT.MATPOWER.FIELD_DROPPED | `"emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band"` | any bus with `evhi` or `evlo` | mp-wr:73-82 |
| EMIT.MATPOWER.VALUE_COLLAPSED | `"{n} branch terminal admittance record(s) collapsed to total susceptance: MATPOWER cannot carry conductance or asymmetric terminal charging"` | count of `has_non_matpower_charging()` > 0 | mp-wr:83-92 |
| EMIT.MATPOWER.FIELD_DROPPED | `"{n} branch current rating record(s) dropped: MATPOWER branch rows carry MVA ratings only"` | any `branch.current_ratings.is_some()` | mp-wr:93-102 |
| EMIT.MATPOWER.VALUE_DEFAULTED | `"{padded} generator(s) with no capability or ramp data written with zeros in columns 11-21: ..."` | some generator `has_caps()` and others do not | mp-wr:109-122 |
| EMIT.MATPOWER.RECORD_DROPPED | `"{idle_loads} out of service load(s) dropped, {p:.4} MW and {q:.4} MVAr: a MATPOWER bus row states one demand with no status, so an idle load would read back as live"` | any load with `!in_service` | mp-wr:127-142 |
| EMIT.MATPOWER.RECORD_DROPPED | `"{idle_shunts} out of service shunt(s) dropped: a MATPOWER bus row states one shunt with no status"` | any shunt with `!in_service` | mp-wr:143-149 |
| EMIT.MATPOWER.RATING_SET_DROPPED | `"branch {i+1} ({from} to {to}) rating set {name}={rate} MVA dropped: MATPOWER .m has no field for branch rating sets beyond rate_a, rate_b, and rate_c"` | one per entry of `branch.rating_sets` | mp-wr:150 via fmt:1584-1598, 1511-1526 |
| EMIT.MATPOWER.FIELD_DROPPED | `"{n} branch solution value set(s) dropped: MATPOWER branch rows do not carry solved flow columns"` | any `branch.solution.is_some()` | mp-wr:151-160 |
| EMIT.MATPOWER.FIELD_DROPPED | `"{n} voltage dependent load model(s) dropped: MATPOWER carries only static Pd/Qd"` | any load whose `voltage_model.has_non_matpower_fields()` | mp-wr:161-174 |
| EMIT.MATPOWER.FIELD_DROPPED | `"gen cost dropped: {with_cost} of {n} generators carry cost data, but MATPOWER's `mpc.gencost` block is all-or-nothing"` | `0 < with_cost < generators.len()` | mp-wr:175-185 |
| EMIT.MATPOWER.EXTRAS_DROPPED | `"{dropped} element(s) carry source-format passthrough fields (extras) the canonical MATPOWER .m writer does not replay; dropped"` | any bus, branch, load, shunt, switch, storage, hvdc, or 3W transformer with nonempty `extras` | mp-wr:190 via fmt:1534-1563 |
| EMIT.MATPOWER.FIELD_DROPPED | `"{lossy} of {n} area record(s) carry a name, source identity, classification, or interchange data: `mpc.areas` holds only the area number and reference bus"` | any area with name, uid, area_type, or nonzero interchange or tolerance | mp-wr:191-208 |

Shared passes run by `emit_value_text` after the MATPOWER serializer (fmt:1127-1131):

| code | message literal | condition | file:line |
|---|---|---|---|
| EMIT.MATPOWER.REFERENCE_MISSING | `"no reference (slack) bus in the source network; power flow tools reject such cases; to_normalized synthesizes a slack at the largest pmax in service generator bus"` | no bus with `kind == Ref`; MATPOWER is in the `needs_ref` set | fmt:1615-1641 |
| EMIT.MATPOWER.FIELD_DROPPED | `"system base frequency {f} Hz dropped: MATPOWER .m has no frequency field (reads back as 60 Hz)"` | `abs(base_frequency - 60) > 1e-9` | fmt:1433-1452 |
| EMIT.MATPOWER.FIELD_DROPPED | `"{n} bus location(s) and {routed} branch route(s) dropped: MATPOWER .m has no coordinate field (...)"` | any `bus.location` or `branch.route` | fmt:1460-1480 |
| `element_relabeled` via `warn_normalized_tap` | never fires for MATPOWER (early return) | | fmt:1655-1658 |
| TRANSFORM.GEN_COST.POLICY_APPLIED | `"generator cost patch applied to {n} generator(s)"` and `"generator cost synthesized for {n} generator(s): model 2, ncost 3, coeffs [...], startup ..., shutdown ..."` | only under non-default `EmitOptions` | fmt:1339-1365 |

Reader (mp-mod, mp-rows, mp-ml): errors only, no warnings. Codes come from `Error::code()` (err:144-170); messages are the `#[error(...)]` strings at err:11-72.

| code | message literal | condition | file:line |
|---|---|---|---|
| PARSE.MATPOWER.MALFORMED (`MissingField`) | `"missing required MATPOWER field `{0}`"` for baseMVA, bus, branch | assignment absent | mp-mod:86, 88, 106 |
| PARSE.MATPOWER.MALFORMED (`BadFloat`) | `"could not parse `{field}` row {row} value `{value}` as f64"` | a token is not Inf, NaN, or a float; `leak_field` maps areas, dclinecost, bus_name to "(unknown)" | mp-ml:52-56, 141-145, 183-195 |
| PARSE.MATPOWER.MALFORMED (`UnbalancedBrackets`) | `"unbalanced brackets in MATPOWER `{0}` matrix"` | `[` without `]` | mp-ml:159-161 |
| PARSE.MATPOWER.MALFORMED (`ShortRow`) | `"malformed MATPOWER `{field}` row {row}: expected at least {expected} columns, got {got}"` | see the column count row above | mp-rows:128-138 |
| PARSE.MATPOWER.MALFORMED (`BadId`) | `"malformed MATPOWER `{field}` row {row}: `{column}` value {v:?} is outside the id range 0..2^63"` | negative, non-finite, or >= 2^63 id value | mp-rows:142-148; fmt:858-872 |
| VALIDATE.GEN_COST.COUNT_MISMATCH | `"`gen` has {gens} rows but `gencost` has {gencost}; expected {gens} (active only) or {2*gens} (active + reactive)"` | gencost rows neither n nor 2n | mp-mod:238-244 |
| VALIDATE.DC_LINE_COST.COUNT_MISMATCH | `"`dcline` has {dclines} rows but `dclinecost` has {dclinecost}; expected one cost row per dcline"` | row counts differ | mp-mod:208-213 |
| PARSE.SOURCE.MALFORMED (`FormatRead { format: "MATPOWER" }`) | `"MATPOWER read error: ..."` for an id outside int64, a duplicate bus id, and every element kind referencing an unknown bus | `check_references` after build | mp-mod:73; net:4011-4099 |
| PARSE.SOURCE.MALFORMED (`FormatRead { format: "MATPOWER .m" }`) | `"MATPOWER .m read error: case has no buses or DC equipment"` | no buses after parse | fmt:828, 877-896 |

Reader behaviors with no diagnostic at all (candidates once a warnings channel exists): unknown bus type code read as PQ (net:1196-1203); `bus_name` with a count mismatch discarded (mp-mod:121); reactive gencost block discarded (mp-mod:245-249); gen columns past 21 and bus or branch columns past 13 discarded (mp-rows:239, 155, 201); `gentype` and `genfuel` never read; `mpc.version` never checked; GEN_STATUS and BR_STATUS values other than exactly 1 read as out of service (mp-rows:14-19, 217, 253).

### 1.4 MATPOWER fixes in priority order

1. Status semantics: `is_in_service` should be `status > 0.0` for gen and branch (MATPOWER manual and PI:317-319, 309-311 agree); mp-rows:17-19. Small.
2. `voltage_regulation_on = vg != 0.0` in `gen_row` (mp-rows:256), matching PI:203; and the writer counterpart: write vg = 0 when regulation is off and warn. Small each.
3. Silent writer drops that need a warning branch in `canonical_warnings`: static VAR compensators, `Shunt.control` and `section_count`, `Branch.control`, generator `regulated_bus` and `regulating_terminal`, HVDC converter metadata. Each small.
4. Three winding transformers: call `expand_transformers_3w` inside `write_matpower` instead of warning (net:3921; mp-wr:63-72). Medium.
5. Current ratings to MVA when `rate_*` is zero (mp-wr:93-102), matching PE:393-395 but with the sqrt(3) factor PowSybl omits. Small.
6. Reader warnings channel (fmt:797), then the `bus_name` mismatch and reactive gencost notices. Medium.
7. `leak_field` (mp-ml:183-195) lacks `areas` and `dclinecost`, so a bad float in those blocks reports "(unknown)". Small.

## 2. PSS/E RAW and RAWX

Aliases for this section:

| alias | path |
|---|---|
| PC | /home/sam/Research/powsybl-core/psse/psse-converter/src/main/java/com/powsybl/psse/converter |
| PM | /home/sam/Research/powsybl-core/psse/psse-model/src/main/java/com/powsybl/psse/model |
| PD | /home/sam/Research/powsybl-core/docs/grid_exchange_formats/psse |

Rules used for the gap column:

- PowSybl reads the whole file into `PssePowerFlowModel` (every field of every record group: PM/pf/io/PowerFlowRawData33.java:30-35, PM/pf/io/PowerFlowRawData35.java:37-44, PM/pf/io/PowerFlowRawxData35.java:52-75) and keeps it as a network extension for update export (PC/PsseImporter.java:163). "PowSybl carries it" means the value reaches the IIDM network. A value that lives only in that extension is marked "model only": update export rewrites it unchanged, full export replaces it with a default.
- PowSybl accepts RAW revisions 32, 33, 35 and RAWX 35; revision 34 is refused (PM/PsseVersion.java:20-24, 82-84, 90-93; PM/pf/PsseCaseIdentification.java:161-164). powerio accepts RAW 33, 34, 35 and RAWX 35 (psse:1708-1729; rawx:338-343).
- RAWX in powerio is a table-to-record translation feeding the same reader (rawx:1-6, 271-471); rows note where RAWX differs.

### 2.1 PSS/E import

| element or attribute | PowSybl (file:line) | powerio (file:line) | gap kind | fix size |
|---|---|---|---|---|
| file revisions | RAW 32/33/35, RAWX 35; 34 rejected (PM/PsseVersion.java:20-24, 90-93; PM/pf/PsseCaseIdentification.java:161-164) | RAW 33/34/35 (psse:1708-1729); RAWX 35 only (rawx:338-343) | powerio does more | none |
| case IC | IC = 1 throws (PM/pf/PsseCaseIdentification.java:158-160) | RAW: field never read, only fields 1, 2, 5 (psse:1869-1908); RAWX warns `READ_PSSE_FIELD_DROPPED` when ic != 0 (rawx:319-329) | powerio drops it (RAW silently) | small |
| SBASE | per unit base (PC/PsseImporter.java:180); must be > 0 (PM/pf/PsseValidation.java:100-103) | `base_mva`, positive finite required (psse:1870-1887) | no gap | none |
| REV | selects the reader layout (PC/PsseImporter.java:147-149) | selects 33/34/35 layouts (psse:1888); unsupported is an error (psse:1722-1728) | powerio does more (34) | none |
| XFRRAT, NXFRAT | model only | RAW ignored; RAWX warns `FIELD_DROPPED` (rawx:319-329) | no gap | none |
| BASFRQ | validated > 0 (PM/pf/PsseValidation.java:104-107), not converted (IIDM has no frequency) | `base_frequency` typed, default 60 (psse:1889-1908) | powerio does more | none |
| TITLE1, TITLE2 | network id = data source base name (PC/PsseImporter.java:158); titles model only | TITLE1 = network name (psse:1910-1915); TITLE2 line discarded (psse:1916); RAWX warns on a nonempty title2 (rawx:330-337) | no gap | none |
| bus I, NAME, BASKV | id `B<n>` (PC/AbstractConverter.java:126-128); name (PC/BusConverter.java:40); nominalV = BASKV, or 1 when 0 or the option is set (PC/VoltageLevelConverter.java:75-77) | `BusId(I)` verbatim (psse:2964-2975); name trimmed (psse:2976-2979); base_kv (psse:2996) | no gap | none |
| bus IDE 1/2/3/4 and slack | IDE not stored; 2 or 3 turns generator regulation on (PC/GeneratorConverter.java:82, 97-99); each IDE 3 bus attaches a `SlackTerminal` to its voltage level (node breaker: the node with the most generators, PC/VoltageLevelConverter.java:190-199; else the first connected terminal, PC/SlackConverter.java:36-57); several IDE 3 buses each keep one; IDE 4 gets no special treatment | kind typed 1..4 (psse:2950-2957, 2993); normalize drops Isolated (norm:781-783), keeps every Ref hosting a generator, else Pv, generator-less Pq (norm:819-830), designates the largest-pmax generator bus when no Ref survives (norm:831-833, 635-662) | no gap | none |
| bus AREA, ZONE, OWNER | AREA is a voltage level grouping key (PC/PsseNodeContainerMapping.java:56-59) and Area membership (PC/AreaConverter.java:50-58); ZONE, OWNER model only | area, zone typed (psse:3001-3002); owner in extras `psse_owner` when != 1 (psse:2986-2990) | powerio does more | none |
| bus VM, VA | V = VM x nominalV, angle = VA (PC/BusConverter.java:42-43); node VM/VA overwrite the bus view (PC/VoltageLevelConverter.java:201-212) | vm, va typed (psse:2994-2995); node VM/VA kept as metadata `psse_vm`, `psse_va` (rawx:1418-1432) | no gap | none |
| bus NVHI, NVLO, EVHI, EVLO | model only; the voltage level is created without limits (PC/VoltageLevelConverter.java:57-61) | vmax, vmin typed; evhi, evlo typed only when distinct (psse:2980-2985, 2997-3000); node breaker voltage levels get limits from vmin/vmax x base_kv (rawx:1404-1411) | powerio does more | none |
| generator PG, QG, PT, PB, QT, QB | targetP, targetQ, maxP, minP (PC/GeneratorConverter.java:41-44); `MinMaxReactiveLimits` (59-62); QT < QB or PT < PB is a validation error (PM/pf/PsseValidation.java:167-178) | pg, qg, qmax, qmin, pmax, pmin (psse:3466-3474) | no gap | none |
| generator VS | targetV = VS x nominalV of the regulating terminal (PC/GeneratorConverter.java:83-84, 92); VS <= 0 on an IDE 2/3 regulated bus is an error (PM/pf/PsseValidation.java:187-195) | vg = VS p.u. (psse:3470) | no gap | none |
| generator IREG, NREG | IREG 0 = own terminal; else node breaker node NREG (or default node), else the first connected terminal of `B<IREG>`; warns when unresolved (PC/GeneratorConverter.java:101-123); a missing IREG bus is reset to 0 (PM/pf/PsseFixes.java:116-123) and is a validation error (PM/pf/PsseValidation.java:171-174) | `regulated_bus` = IREG when nonzero, an explicit same-bus IREG kept (psse:3417-3419, 3479); NREG (RAW 35, RAWX) resolved to a `TerminalReference` (psse:1745-1825; rawx:1154-1168); a missing bus is dropped with `READ_PSSE_REFERENCE_DROPPED` (psse:2516-2527) | no gap | none |
| generator voltage regulator on/off | on iff bus IDE is 2 or 3, targetV > 0, QB < QT (PC/GeneratorConverter.java:86-94) | `voltage_regulation_on` always true (psse:3477; net:2899) | powerio defaults it | small |
| generator MBASE | model only; full export writes the system base (PC/GeneratorConverter.java:171) | mbase typed (psse:3471), written (psse:549) | powerio does more | none |
| generator ZR, ZX, RT, XT, GTAP | RT or XT != 0 warns "Implicit method ... not supported" (PC/GeneratorConverter.java:64-66); all model only; defaults on export (PC/AbstractConverter.java:692-696) | generator component metadata `psse_zr` and the others when non-default (psse:3430-3439, 3488-3543), replayed (psse:498-502, 1539-1591) | powerio does more; PowSybl warns too | none |
| generator RMPCT | model only; export 100 (PC/AbstractConverter.java:698) | metadata `psse_rmpct` (psse:3436), replayed (psse:503) | powerio does more | none |
| generator STAT | connected iff STAT == 1 (PC/GeneratorConverter.java:54) | in_service = STAT != 0 (psse:3472) | no gap | none |
| generator O1..F4 | model only; defaults (PC/AbstractConverter.java:702, 725-736) | metadata `psse_o1`..`psse_f4` (psse:3444-3459), replayed (psse:505-522) | powerio does more | none |
| generator WMOD, WPF | model only; export 0 and 1.0 (PC/AbstractConverter.java:703-704) | metadata (psse:3460-3461), replayed (psse:523-524) | powerio does more | none |
| generator BASLOD (35) | model only; export 0 (PC/AbstractConverter.java:701) | metadata (psse:3441-3443), written at 35 (psse:560-562), warned below 35 (psse:525-533) | powerio does more | none |
| generator ID | id `B<n>-G<id>` (PC/AbstractConverter.java:142-144); duplicates: last wins with a warning (PM/pf/PsseFixes.java:90-107) and a validation error (PM/pf/PsseValidation.java:184, 636-643) | ID kept as `psse_eqid` metadata when it differs from the positional id (psse:3379-3405), replayed (psse:469-472) | no gap | none |
| generator cost | none in IIDM, nothing reported | cost None (psse:3475); the writer warns when any generator has a cost (psse:1256-1261); normalization warns `CANONICALIZE.NORMALIZE.GEN_COST_ABSENT` (norm:836-845); `--missing-gen-cost` documented (docs/src/format-fidelity.md:336-343) | powerio does more (declares the absence) | none |
| load PL, QL, IP, IQ, YP, YQ | p0 = PL + IP + YP, q0 = QL + IQ + YQ (PC/LoadConverter.java:38-39); `ZipModel` fractions when I or Y is nonzero (47-81) (PD/import.md:66 says discarded; the code builds the model); full export writes PL/QL only (110-111) | p, q sums (psse:3173-3174); `LoadVoltageModel::Zip` in source units (psse:3149-3161; net:2108-2124); components in extras `psse_pl` and the rest (psse:3108-3119); the writer replays them (psse:391, 4334-4416) | powerio does more | none |
| load SCALE | model only; export 1 (PC/AbstractConverter.java:646) | typed `scaling` when != 1 (psse:3147, 3160), extras `psse_scal` (psse:3120-3129), written (psse:393-395) | powerio does more | none |
| load INTRPT | model only; export 0 (PC/AbstractConverter.java:647) | extras `psse_intrpt` (psse:3123) with `READ_PSSE_RETAINED_SOURCE_ONLY` (psse:3162-3170), written (psse:396) | powerio does more | none |
| load DGENP, DGENQ, DGENM (35; PDGEN/QDGEN/STDG in 34) | model only; export 0 (PC/AbstractConverter.java:648-650) | extras `psse_pdgen`, `psse_qdgen`, `psse_flagstatus` for rev >= 34 (psse:3130-3141), written (psse:407-434) | powerio does more | none |
| load LOADTYPE (35) | model only; export "" (PC/AbstractConverter.java:651) | extras `psse_loadtype` (psse:3142-3146), typed `load_type` only when it parses as an integer (psse:3148); written (psse:411-426); below 35 warns `record_dropped` (psse:398-406) | powerio does more | none |
| load AREA, ZONE, OWNER | model only; export 1 (PC/AbstractConverter.java:637-638, 645) | extras only when they differ from the bus (psse:3098-3103, 3073-3085), owner (psse:3121); written (psse:384-392) | powerio does more | none |
| load STATUS, ID | connected iff STATUS == 1 (PC/LoadConverter.java:90); id `B<n>-L<id>` (PC/AbstractConverter.java:150-152) | in_service (psse:3176); extras `id` when != "1" (psse:3016-3026, 3097) | no gap | none |
| fixed shunt GL, BL | linear model, per section g = GL/vnom^2 (PC/FixedShuntCompensatorConverter.java:48-52); GL = BL = 0 skipped with a warning (37-40) | g, b in MW/MVAr at 1 p.u. (psse:3184-3193); zero shunts kept | PowSybl warns too | none |
| switched shunt MODSW | regulates only for MODSW 1 or 2 (PC/SwitchedShuntCompensatorConverter.java:111-113); 3..6 model only | Locked, Continuous, Discrete (psse:3298-3303); the original code kept in `psse_modsw` when not canonical (psse:3263-3265), written (psse:1139-1141) | powerio does more | none |
| switched shunt ADJM | ADJM 1 sorts blocks and warns "Switched combination not exactly supported" (PC/SwitchedShuntCompensatorConverter.java:172-180); export 0 (396) | extras `psse_adjm` (psse:3269-3271), written (psse:1142); no warning | PowSybl warns too | none |
| switched shunt STAT | connected iff STAT == 1 (PC/SwitchedShuntCompensatorConverter.java:60) | in_service (psse:3286) | no gap | none |
| switched shunt VSWHI, VSWLO | targetV = 0.5(VSWLO + VSWHI) x vnom, deadband = the difference (PC/SwitchedShuntCompensatorConverter.java:94-101) | vhigh, vlow typed (psse:3248-3249) | no gap | none |
| switched shunt SWREM/SWREG, NREG | regulating terminal (PC/SwitchedShuntCompensatorConverter.java:116-146; NREG at 123, the comment at 115 says not considered); a missing bus is reset (PM/pf/PsseFixes.java:172-180) and a validation error (PM/pf/PsseValidation.java:514-517) | control_bus typed (psse:3212, 3250); NREG resolved (psse:3273-3280, 1762-1773); a missing bus dropped with a warning (psse:2570-2584) | no gap | none |
| switched shunt RMPCT | model only; export 100 (PC/SwitchedShuntCompensatorConverter.java:402) | rmpct typed (psse:3252), written (psse:1179) | powerio does more | none |
| switched shunt RMIDNT | model only; export "" (403) | extras `psse_rmidnt` (psse:3272), written (psse:1143-1151) | powerio does more | none |
| switched shunt BINIT | sectionCount = the section closest to BINIT (PC/SwitchedShuntCompensatorConverter.java:51, 148-159); export BINIT = current B (303) | b = BINIT (psse:3285); section_count None (psse:3287) | no gap | none |
| switched shunt N1/B1..N8/B8 | nonlinear sections: reactor blocks then capacitor blocks accumulated, a zero section inserted, sorted by B (162-211); blocks with N = 0 or B = 0 skipped (239-244); export collapses sections into blocks, block 8 averaged (332-389) | blocks typed (steps, b) up to the first (0, 0) pair (psse:3216-3244), written (psse:1159-1168) | powerio does more | none |
| switched shunt S1..S8 block status (35) | out-of-service blocks excluded from the sections (216-224, 239-244); the model keeps them | `ShuntBlock` has no status (net:2225-2231); the reader warns `READ_PSSE_FIELD_DROPPED` and keeps the block enabled (psse:3221-3231); the writer emits S = 1 (psse:1161-1165) | powerio warns where PowSybl carries it | medium (`ShuntBlock.in_service: bool`) |
| switched shunt ID (35) | part of the IIDM id, "1" below 35 (270-276) | extras `id` (psse:3255-3259) | no gap | none |
| branch I, J, CKT | id `L-i-j-ckt` (PC/AbstractConverter.java:146-148); duplicates: last wins (PM/pf/PsseFixes.java:51), validation error (PM/pf/PsseValidation.java:212, 645-652) | from, to (psse:3561-3562); CKT in extras `id` when != "1" (psse:3573) | no gap | none |
| branch R, X | ohms with vnom1 x vnom2 / sbase (PC/LineConverter.java:61-62; PC/AbstractConverter.java:490-492); X = 0 replaced by 1e-4 with a warning (PM/pf/PsseFixes.java:195-202) | r, x p.u. (psse:3581-3582); zero X kept | no gap (PowSybl substitutes) | none |
| branch B, GI, BI, GJ, BJ | g1/b1/g2/b2 with B/2 per end (PC/LineConverter.java:64-71; PC/AbstractConverter.java:498-503); warns "Branch G not supported" when GI or GJ != 0 although the value is mapped (PC/LineConverter.java:109-111) | charging g_fr = GI, b_fr = B/2 + BI, and the same at the other end (psse:3554-3560, 3583-3589) | no gap (PowSybl's warning is stale) | none |
| branch RATEA/RATEB/RATEC (33), RATE1..RATE12 (34/35) | only RATEA (33) or RATE1 (35) becomes a permanent `CurrentLimit` per end, A = 1000 x MVA / (sqrt(3) x vnom) (PC/LineConverter.java:114-137); RATEB, RATEC, RATE2..12 model only; no temporary limits | rate_a/b/c in MVA (psse:3590-3592); RATE4..12 as `rating_sets` named RATE4..RATE12 (psse:3593, 59-75); written (psse:619-630); the 33 writer drops extra sets with `rating_set_dropped` (psse:138-140, 1354-1356) | powerio does more | none |
| branch ST | connected iff ST == 1 (PC/LineConverter.java:95, 103) | in_service (psse:3597) | no gap | none |
| branch MET, LEN | model only; export 1 and 0 (PC/AbstractConverter.java:670-671) | extras `psse_met`, `psse_len` (psse:3574-3575), written (psse:595-596) | powerio does more | none |
| branch O1..F4 | model only; defaults (PC/AbstractConverter.java:672) | extras (psse:3576, 4273-4291), written (psse:597, 637-644) | powerio does more | none |
| branch NAME | 35: NAME; 32/33: synthesized `<busIname>_<busJname>_<ckt>` (PC/LineConverter.java:208-221) | name typed for rev >= 34 named records (psse:3563-3572); None for 33 | no gap | none |
| transformer CW, CZ, CM | 1/2/3 and 1/2 converted; any other code throws (PC/TransformerConverter.java:296-319, 321-346, 352-359); CM 2 with a negative b^2 warns and sets 0 (336-341) | same conversions (psse:2255-2302, 2305-2360, 2399-2436); unknown codes warn `READ_PSSE_VALUE_UNSUPPORTED` and read as p.u. (psse:2295-2300, 2353-2358, 2429-2434); bad bases warn `VALUE_SUBSTITUTED` (psse:2269-2285, 2333-2338, 2412-2417); fresh output is CW = CZ = CM = 1 (psse:768-770) | PowSybl warns too | none |
| transformer MAG1, MAG2 | Ysh moved past the ratio into G/B with per-step corrections (PC/TransformerConverter.java:83-98, 532-541); 3W: leg 1 only (191, 218-219) | from-side charging g_fr/b_fr (psse:3745-3751); 3W mag_g/mag_b (psse:4022-4023) | no gap | none |
| transformer NMETR, VECGRP, ZCOD (35) | model only; export 2, blank, 0 (PC/TransformerConverter.java:1160, 1164, 1165) | extras `psse_nmetr`, `psse_vecgrp`, `psse_zcod` (psse:3649-3654), written (psse:751, 742-750, 782-784); ZCOD below 35 warned (psse:754-762) | powerio does more | none |
| transformer NAME, STAT (2W) | name (PC/TransformerConverter.java:258-264); connected iff STAT == 1 (124, 132) | name (psse:3736-3740); in_service (psse:3759) | no gap | none |
| transformer R1-2, X1-2, SBASE1-2 | to system base per CZ, then ohms on the VL2 nominal (PC/TransformerConverter.java:75, 86, 296-319); export sbase12 = 100 (899) | system-base r, x (psse:3694-3705); SBASE1-2 survives as `control.mva_base` when a control block exists (psse:3889), the writer uses it else base_mva (psse:789-791) | no gap | none |
| transformer WINDV1, WINDV2, NOMV1, NOMV2 | ratios per CW (78-79, 352-359); w2 moved to side 1 (90-91); ratedU = VL nominal (109-110) | tap = ratio1 / ratio2 (psse:3706, 2439-2460); NOMV consumed by the CW 3 and CM 2 conversions (psse:2421-2427, 3726); the writer emits NOMV 0 (psse:830) | no gap (both fold NOMV) | none |
| transformer ANG1 sign | `PhaseTapChanger` alpha = -ANG (PC/TransformerConverter.java:510); export negates back (1315-1317) | shift = ANG in degrees (psse:3758) | no gap | none |
| transformer winding RATES | winding 1 RATA1 (33) or RATE1 (35) into permanent current limits at both ends (PC/TransformerConverter.java:543-561, 590-598); the rest model only | rate_a/b/c and rating sets (psse:3752-3755) | powerio does more | none |
| transformer COD1 magnitude and sign | uses abs(COD) (PC/TransformerConverter.java:410-420, 784-788, 819), so a negative (disabled) COD imports as regulating; COD 2 warns "Reactive power control not supported" (784-787); COD 4/5 yield no control and a single step (404-407); COD 1 with CONT 0 reset to 0 (PM/pf/PsseFixes.java:109-114) | mode from abs(COD) 1..5 (psse:3775-3788); `enabled` = COD > 0 (psse:3880); an unknown magnitude warns `VALUE_UNSUPPORTED` (psse:3824-3831) (net:2743-2756, 2770-2793) | powerio does more | none |
| transformer CONT1, NODE1 | terminal from abs(CONT) (PC/TransformerConverter.java:851-870), NODE in node breaker (857); a missing bus is reset (PM/pf/PsseFixes.java:137-143); validation error when abs(COD) = 1 and CONT is invalid (PM/pf/PsseValidation.java:383-388) | `controlled_bus` and `controlled_bus_on_winding_side` from the sign (psse:3821, 3881-3882); NODE resolved to a terminal (psse:1774-1787); a stale pointer dropped with a warning (psse:2529-2545) | powerio does more | none |
| transformer RMA1, RMI1, NTP1 (tap table) | NTP evenly spaced steps RMI..RMA (ratio steps for COD 1/2 with zero angle, angle steps for COD 3), the current ratio inserted as a step, tapPosition = its index (PC/TransformerConverter.java:370-450); rho = 1/ratio (493) | tap_min/tap_max (CW converted for voltage and reactive modes) and ntp typed (psse:3832-3866); no step table (`DetailedConnectivity.tap_changers` stays empty for PSS/E) | no gap (PowSybl derives its table from the same three numbers) | none |
| transformer VMA1, VMI1 | targetV and deadband (COD 1) (797-801) or regulation value and deadband (COD 3) (828-831) | band_max, band_min (psse:3864-3865) | no gap | none |
| transformer TAB1 and impedance correction tables | model only; tables read (PM/pf/io/PowerFlowRawData35.java:40) but not applied (PD/import.md:190) | extras `psse_tab` (psse:3665); the table section skipped with `READ_PSSE_SECTION_UNSUPPORTED` (psse:2500-2510), RAWX `impcor` (rawx:100) | no gap | none |
| transformer CR1, CX1 | model only; export 0 (PC/TransformerConverter.java:1202-1203) | extras `psse_cr`, `psse_cx` (psse:3666-3667), written (psse:820-821) | powerio does more | none |
| transformer CNXA1 (34/35) | model only; export 0 (1204) | typed `winding_connection_angle` when abs(COD) = 5 or nonzero (psse:3867, 3890-3892), written (psse:816-818) | powerio does more | none |
| 3W transformer model | star with ratedU0 = 1 kV; z1 = 0.5(z12 + z31 - z23) and cyclic (PC/TransformerConverter.java:177-179, 194-200); ratedU per leg = VL nominal (220, 228, 236) | typed `Transformer3W` with pairwise z on the system base and declared bases (psse:3926-3946; net:3387-3406); star lowered in the indexed view (`/home/sam/Research/powerio/powerio-tx/src/indexed.rs:116, 131`; net:3449, 3921) | no gap | none |
| 3W VMSTAR, ANSTAR | string properties "v" and "angle" (PC/TransformerConverter.java:251-252) | star_vm, star_va typed (psse:4020-4021) | powerio does more | none |
| 3W STAT 2/3/4 (one winding out) | per-leg connection (PC/TransformerConverter.java:240-248, 284-294); export recomputes 1..4 (1265-1289) | in_service = STAT != 0 (psse:4026); the writer emits 1 or 0 (psse:932) | powerio drops it | medium (per-winding status on `Winding` or a `status: u8` on `Transformer3W`) |
| 3W winding controls (one per winding) | only the first enabled control is kept; later ones forced off with a warning (PC/TransformerConverter.java:756-780, 804-807) | every winding keeps its control (psse:3947-3996) | powerio does more | none |
| 3W winding COD 4 | no control, silent (PC/TransformerConverter.java:397-407) | a hard read error for the whole case (psse:3967-3977) | powerio drops it (as an error) | small |
| two-terminal DC NAME, MDC, RDC, SETVL, VSCHD | LCC stations plus `HvdcLine` (PC/TwoTerminalDcConverter.java:35-87): r = RDC (79), nominalV = VSCHD (80), setpoint abs(SETVL) (MDC 1) or SETVL x VSCHD / 1000 (MDC 2) else 0 (89-98), maxP 1.2 x setpoint (100-102), connected iff MDC != 0 (54, 72), side 1 rectifier (83); the sign of SETVL is lost (93) | from = IPR bus, to = IPI bus (psse:4176-4177); in_service = MDC != 0 (psse:4178); pf/pt priced by I^2 RDC per mode (psse:4080-4131); name, mdc, rdc, vschd, inverter flag, unpriceable amps in extras (psse:4142-4165); `resistance_ohm`, `nominal_voltage_kv` left None (psse:4193-4194; net:3151-3156) | powerio defaults it (typed ohm and kV fields) | small |
| two-terminal DC VCMOD, RCOMP, DELTI, METER, DCVMIN, CCCITMX, CCCACC | model only; export defaults (PC/TwoTerminalDcConverter.java:126-132) | control tail kept verbatim in `psse_dc_control_tail` when non-default (psse:4166-4174, 4209-4212), replayed (psse:1090) | powerio does more | none |
| two-terminal DC converter records (IPR, NBR, ANMXR, ANMNR, RCR, XCR, EBASR, TRR, TAPR, TMXR, TMNR, STPR, ICR, NDR, IFR, ITR, IDR, XCAPR) | IP = station bus; ANMX becomes powerFactor = 0.5(cos ANMX + cos 60) (PC/TwoTerminalDcConverter.java:104-107); loss factor 0 (39); every other field model only, defaults on export (163-184) | tails kept verbatim `psse_dc_rectifier_tail`, `psse_dc_inverter_tail` (psse:4166-4174), replayed (psse:1091-1092, 1122-1123); no typed `HvdcConverter` (converter1/2 None, psse:4196-4197; type at net:3113-3128); `DetailedConnectivity.line_commutated_converters` never filled | powerio does more on retention; powerio defaults it on the typed converter | medium (fill `HvdcConverter { kind: Lcc, power_factor }` from ANMX; fill `LineCommutatedConverter` from the tail) |
| VSC DC line (NAME, MDC, RDC; converters IBUS, TYPE, MODE, DCSET, ACSET, ALOSS, BLOSS, MINLOSS, SMAX, IMAX, PWF, MAXQ, MINQ, REMOT/VSREG, NREG, RMPCT) | `VscConverterStation` pair plus `HvdcLine` (PC/VscDcTransmissionLineConverter.java:39-98): setpoint abs(DCSET) of the TYPE 2 converter (149-157), mode by the DCSET sign (167-175), lossFactor from ALOSS (100-120), Q setpoint from ACSET as a power factor when MODE != 1 (122-128), MAXQ/MINQ only when MODE 1 (130-143), maxP from SMAX/IMAX (159-165), nominalV = max VL nominal (145-147), voltage control ACSET p.u. when MODE 1 (177-228), REMOT (33) or VSREG (35) (210-216), NREG unused (199); BLOSS, MINLOSS, PWF, RMPCT model only | not read: the RAW section is skipped with `READ_PSSE_SECTION_UNSUPPORTED` (psse:2500-2510; not in psse:2180-2196); RAWX `vscdc` warned (rawx:99, 761-777); the writer emits an empty section (psse:1675) | powerio drops it | large (reader and writer; `Hvdc.converter1/2` with `HvdcConverterKind::Vsc` already exist, net:3104-3128) |
| multi-terminal DC | model only, no converter, silent (PM/pf/io/PowerFlowRawData35.java:42; PM/pf/io/PowerFlowRawxData35.java:63) | skipped with a warning (psse:2500-2510); RAWX `ntermdc*` (rawx:101-104) | no gap (powerio warns, PowSybl is silent) | none |
| FACTS | a STATCOM (J = 0) becomes a `StaticVarCompensator`: bmax = SHMX/vnom^2, VSET voltage or QDES reactive mode, REMOT/FCREG terminal (PC/FactsDeviceConverter.java:35-131); series devices (J != 0) silently skipped (39-41, 208-210) | skipped with a warning (RAW) and RAWX `facts` (rawx:109); `StaticVarCompensator` exists in the model (net:2296-2314) but the PSS/E reader never fills it (psse:2110) | powerio drops it | medium |
| GNE devices, induction machines, multi-section line groups, zone names, owner names, inter-area transfers | model only, no converter, silent | sections skipped with `READ_PSSE_SECTION_UNSUPPORTED` (psse:2500-2510); RAWX tables (rawx:105-111); the bus zone number itself is typed (psse:3002) | no gap (powerio warns, PowSybl is silent) | none |
| areas (I, ISW, PDES, PTOL, ARNAME) | `Area` id `A<n>`, type ControlArea, interchangeTarget = PDES, name = ARNAME, voltage levels by bus AREA, edge terminals derived from lines, HVDC, transformers (PC/AreaConverter.java:35-118); ISW, PTOL model only | `Area` typed: number, slack_bus = ISW, net_interchange, tolerance, name, area_type "ControlArea" (psse:3316-3331; net:3234-3251); a stale ISW dropped with a warning (psse:2586-2597); normalization drops areas (norm:864-866) | powerio does more (ISW, PTOL) | none |
| system switching devices (35 SYSTEM SWITCHING DEVICE, RAWX `sysswd`) | the section is skipped on read and written empty (PM/pf/io/PowerFlowRawData35.java:51, 71); the RAWX reader has no `sysswd` table (PM/pf/io/PowerFlowRawxData35.java:52-75) | `Switch` typed: from, to, closed = STAT != 0, thermal_rating = RATE1, extras ckt, xpu, rate2..12, nstat, met, stype, name (psse:3609-3642), RAWX (rawx:779-834) | powerio does more | none |
| substation records (35; also 34 in powerio) | 35 only (PC/NodeBreakerValidation.java:49-51); a substation is valid only if every switch-connected node set maps to one bus (126-141), a bus is in one substation (63-67), every equipment terminal of its buses is listed (70-105), control nodes exist (172-180); invalid substations silently fall back to bus breaker; valid ones give NODE_BREAKER voltage levels (PC/VoltageLevelConverter.java:54-65); ignore option (PC/PsseImporter.java:57-60) | RAW rev >= 34 (psse:2122-2124) via rawx:1669-1854, RAWX rawx:1265-1664: substations, voltage levels keyed by (isub, base_kv) (rawx:1378-1379), nodes (rawx:1412-1417), switches, terminals, busbar sections; dangling references are hard errors (rawx:1366-1370, 1375-1377, 1469-1483, 1541-1545); no terminal completeness check; terminals to unmapped equipment get a placeholder with `READ_PSSE_RETAINED_SOURCE_ONLY` (rawx:1557-1568) | powerio does more (34 supported, no silent fallback) | none |
| substation IS, NAME, LATI, LONG, SRG | name copied (PC/SubstationConverter.java:41-44); the rest model only; export 0, 0, 0.1 (PC/VoltageLevelConverter.java:533-535) | metadata name and `psse_isub`, `psse_lati`, `psse_long`, `psse_srg` (rawx:1327-1349), written (rawx:2670-2680) | powerio does more | none |
| substation node NI, NAME, I, STATUS, VM, VA | VM/VA applied to the bus view (PC/VoltageLevelConverter.java:201-212); node name on busbar sections (156); STATUS model only | node_number, calculated_bus typed (rawx:1412-1417); name, `psse_stat`, `psse_vm`, `psse_va` metadata (rawx:1418-1445), written (rawx:2779-2791) | no gap | none |
| switching device NI, NJ, CKT, NAME, TYPE, STATUS, NSTAT, X, RATE1..3 | kind: TYPE 2 breaker, else disconnector (PC/VoltageLevelConverter.java:186-188); open = STATUS != 1 (112); id `<VL>-Sw-ni-nj-ckt` (PC/AbstractConverter.java:194-196); NSTAT, X, RATE model only | same kind and open rules (rawx:1493-1507); `psse_swdid`, type, stat, nstat, xpu, rate1..3, rsetnam metadata (rawx:1508-1516), written (rawx:2827-2856) | powerio does more | none |
| equipment terminal I, NI, TYPE, ID, J, K | node-independent equipment key (PC/AbstractConverter.java:289-309), mapped per bus (PC/VoltageLevelConverter.java:119-148); a second device on a node gets an internal connection (139-142); equipment-free nodes become busbar sections (151-158) | same key (rawx:837-860, 1550-1616); terminal number by bus position (rawx:1569-1580); busbar sections for equipment-free nodes (rawx:1619-1659) | no gap | none |
| element ids and names | `B<n>`, `VL<n1-n2..>`, `S<n1-..>` or `Sub<isub..>`, `B<n>-G<id>`, `B<n>-L<id>`, `B<n>-SH<id>`, `B<n>-SwSH<id>`, `L-i-j-ckt`, `T-i-j[-k]-ckt`, `TwoTerminalDc-<name>`, `VscDcTransmissionLine-<name>`, `FactsDevice-<name>`, `A<n>` (PC/AbstractConverter.java:81-213; PC/AreaConverter.java:36) | `Bus.id` = PSS/E number; `Bus.name` = NAME; `Bus.uid` None until `assign_missing_component_ids` (net:1890) runs (generator metadata psse:3495, RAWX rawx:465); device ids in extras `id` or `psse_eqid`; switch uid `from-to-ckt` (psse:3621) | powerio does more (numbers survive across formats) | none |
| records naming a missing bus | validation warning, record ignored (PM/pf/PsseValidation.java:135, 149, 164, 217-222, 263-268, 407-412) | `check_references` returns an error for the whole case (psse:2129; net:4000) | powerio drops it (the whole case) | small |
| duplicated (bus, id) records | last wins with a warning (PM/pf/PsseFixes.java:90-107); validation error (PM/pf/PsseValidation.java:636-652) | every record kept; the writer reallocates a free id (fmt:1375-1394) | no gap | none |
| RAWX unknown columns, unknown tables, non-numeric strings | ignored silently (PM/pf/io/PowerFlowRawxData35.java:52-75 reads named tables only) | `READ_PSSE_FIELD_DROPPED` per unknown column (rawx:241-259); `READ_PSSE_SECTION_UNSUPPORTED` per unknown member (rawx:720-745); numeric strings validated (rawx:623-638) | powerio does more | none |

### 2.2 PSS/E export

| element or attribute | PowSybl (file:line) | powerio (file:line) | gap kind | fix size |
|---|---|---|---|---|
| export modes | update (default, needs the imported model, same revision and RAW/RAWX form, values patched in place) (PC/PsseExporter.java:45-48, 82-94, 150-172); full export from any network (174-197); new equipment unsupported in update mode (147-149) | an unchanged same-format same-revision source is returned verbatim (fmt:1062-1081); otherwise fresh 33/34/35 or RAWX (fmt:1084-1103); no update mode | powerio drops it (after any edit, the VSC, MTDC, FACTS, zone, owner, impedance table, GNE, and induction machine records the update path preserves are gone) | large |
| revision written | full: 35 only (PC/PsseExporter.java:203); update: 32/33/35 (118-144); RAWX only at 35 (97-107) | 33, 34, 35 and RAWX 35 (fmt:1088-1103); `EMIT_PSSE_DOWNGRADED` when the source revision exceeds the target (fmt:1403-1427) | powerio does more | none |
| case identification | IC 0, SBASE 100, REV 35, XFRRAT 0, NXFRAT 0, BASFRQ 50.0 hard-coded, TITLE1 = RFC 1123 date plus name, TITLE2 "" (PC/PsseExporter.java:199-213) | IC 0, SBASE = base_mva, REV, XFRRAT 0, NXFRAT 1 for 34/35 else 0, BASFRQ = base_frequency, TITLE1 = name (psse:301-310); RAWX `caseid` (rawx:1910-1916) | powerio does more | none |
| system-wide block (34/35) | the empty end marker only (PM/pf/io/PowerFlowRawData35.java:62-66) | GENERAL/NEWTON/SOLVER lines from `SolverParams` (psse:311-343); RAWX drops them with `field_dropped` (rawx:1894-1899) | powerio does more | none |
| bus numbers | full: fresh sequential numbers in voltage level order (PC/VoltageLevelConverter.java:232-237, 278-287, 355; PC/ContextExport.java:113-115); update: recovered from `B<n>` ids (PC/VoltageLevelConverter.java:296-303; PC/AbstractConverter.java:130-136) | `Bus.id` verbatim (psse:359-361) | powerio does more | none |
| bus IDE | recomputed: 4 outside the main connected component, 3 on the slack terminal bus, 2 when a generator regulates its own bus with the regulator on, else 1 (PC/AbstractConverter.java:447-464); update keeps the type in node breaker (PC/BusConverter.java:97-100), isolated gets VM 0 and IDE 4 (116-120) | the stored kind (psse:363, 1417-1419); `reference_missing` when no Ref (fmt:1615-1631) | no gap | none |
| bus VM, VA, NVHI, NVLO, EVHI, EVLO, NAME, AREA, ZONE, OWNER | VM from V/nominalV (1.0 if absent), VA, limits from the voltage level limits or 1.1/0.9 (PC/BusConverter.java:71-84; PC/AbstractConverter.java:538-562); name cut to 12, a leading "-" replaced (627-630); AREA, ZONE, OWNER = 1 (614-616) | typed vm, va, vmax, vmin, evhi, evlo; name sanitized (quote, slash) and padded (psse:350-372); area, zone typed; owner from extras (psse:349) | powerio does more | none |
| loads | PL/QL from the solved terminal P/Q else p0/q0 (PC/LoadConverter.java:103-147); IP/IQ/YP/YQ always 0 (ZIP dropped); other columns default (PC/AbstractConverter.java:632-653); batteries written as loads with -targetP/-targetQ (PC/BatteryConverter.java:37-56) | ZIP components, SCAL, INTRPT, DG, LOADTYPE (psse:376-450); storage dropped with `record_dropped` (psse:1247-1255) | powerio does more on loads; powerio drops it for storage | small (write `Storage` as a load record with -ps/-qs, as PowSybl does for batteries) |
| fixed shunts | GL/BL = g/b of the current section x vnom^2 (PC/FixedShuntCompensatorConverter.java:74-110); a shunt counts as fixed by an id tag or one non-regulating section (PC/AbstractConverter.java:215-225) | `control.is_none()` (psse:452-464) | no gap | none |
| switched shunts | MODSW 1/0, ADJM 0, VSWHI/VSWLO from targetV and deadband, SWREG, NREG, BINIT = current B, RMPCT 100, RMIDNT "", blocks from the linear or nonlinear model (PC/SwitchedShuntCompensatorConverter.java:290-431) | typed control plus `psse_modsw`, `psse_adjm`, `psse_rmidnt` (psse:1129-1195); S = 1 for every block at 35 (psse:1161-1165) | powerio does more | none |
| generators | PG/QG solved or target, QT/QB from the reactive limits at targetP, VS = targetV/vnom, IREG/NREG, MBASE = 100, STAT, PT/PB with 9999 for NaN, other columns default (PC/GeneratorConverter.java:125-176) | typed plus generator metadata (psse:467-571); energy source, caps, cost dropped with warnings (psse:142-174, 1391-1396, 1256-1261) | powerio does more | none |
| lines | R/X/G/B to p.u. with both nominal voltages (PC/LineConverter.java:147-169); ratings: apparent power limits (end 1, then 2), else current limits converted to MVA, else active power limits, sorted ascending into RATE1..12 (171-192; PC/AbstractConverter.java:564-606); NAME 40 chars, MET 1, LEN 0, owners default; tie lines as branches (PC/TieLineConverter.java:31-58); an unpaired dangling line as bus plus branch plus load plus generator (PC/BoundaryLineConverter.java:32-155) | rate_a/b/c keep their slots, rating sets to RATE4..12 (psse:598-673); current ratings, angle limits, solutions dropped with warnings (psse:1296-1305, 1269-1274, 1367-1376) | no gap on lines; current ratings: powerio warns where PowSybl carries it | small (convert a current rating to MVA when the MVA slot is zero) |
| two-winding transformers | CW = CZ = CM = 1 (PC/TransformerConverter.java:1155-1157); R/X on the VL2 nominal (896-898); MAG from G/B with step corrections (886, 924-946); WINDV1 = ratedU ratio x tap ratio, ANG = -alpha, COD/CONT/NODE/RMA/RMI/NTP from the tap changer (906-918, 1071-1113); VMA/VMI always 1.1/0.9, TAB/CR/CX/CNXA 0, NMETR 2, VECGRP blank, ZCOD 0 (1188-1206) | typed control with band, CNXA, COD sign, CONT sign, NTP (psse:786-870); extras replayed (psse:737-762, 819-821) | powerio does more | none |
| three-winding transformers | star back to pairwise on ratedU0 (PC/TransformerConverter.java:989-1004); STAT from the leg statuses (986, 1265-1289) | typed record (psse:875-1053); STAT 1/0 only (psse:932) | powerio drops it (STAT 2/3/4) | medium (see the import row) |
| two-terminal DC | name from the id (12 chars), MDC, RDC, SETVL = setpoint, VSCHD = nominalV, other columns default, converters IP plus ANMX from powerFactor (PC/TwoTerminalDcConverter.java:118-184) | retained tails replayed (psse:1072-1124); `value_defaulted` when the line states more than the record (psse:1239-1246); `HvdcConverter` fields (power factor, loss factor, kind) never read by the writer | powerio drops it (typed converter fields from other sources, silently) | medium |
| VSC DC | VSC records from `VscConverterStation` pairs (PC/VscDcTransmissionLineConverter.java:241-318) | every `Hvdc` written as a two-terminal record regardless of `converter1.kind` (psse:1076-1124); the VSC section is always empty (psse:1675); no warning | powerio drops it (collapses VSC to LCC silently) | large |
| FACTS | SVCs written as STATCOM FACTS records (PC/FactsDeviceConverter.java:133-180) | no FACTS output (psse:1682); `static_var_compensators` is never inspected by the writer (only psse:2110 in the reader) | powerio drops it (silently) | medium |
| areas | full export writes no area records (no `AreaConverter` call in PC/PsseExporter.java:174-197); update keeps the model areas | area records written (psse:1054-1070) | powerio does more | none |
| zones, owners, inter-area transfers, impedance tables, MTDC, GNE, induction machines | full export: empty sections; update: model records rewritten | empty sections (psse:1673-1687) | no gap for fresh output (see the export modes row for update) | none |
| system switching devices | always written empty (PM/pf/io/PowerFlowRawData35.java:71) | `Switch` records at 35 (psse:676-698), RAWX (rawx:2133-2211); dropped with a warning at 33/34 (psse:701-709) | powerio does more | none |
| substation data | one PSS/E substation per IIDM substation for NODE_BREAKER voltage levels with switches and <= 998 nodes (PC/AbstractConverter.java:468-472; PC/VoltageLevelConverter.java:331-387, 495-621); fresh node numbers; switch X 0.0001, rates 0, NSTAT 1 (579-583); RAWX by option (PC/PsseExporter.java:84-88) | from `DetailedConnectivity` at 34/35 and RAWX (psse:1199-1227; rawx:2617-2980, 2984-3083); dropped at 33 with a warning (psse:1228-1235) | powerio does more (34) | none |
| internal connections in node breaker export | folded onto a representative node (PC/VoltageLevelConverter.java:401-441) | dropped with `record_dropped` (rawx:2389, 2418-2426) | powerio warns where PowSybl carries it | medium (merge the two nodes onto one PSS/E node before writing) |
| circuit ids and names | ckt "%02d" sequential per (type, buses), IIDM ids ignored (PC/ContextExport.java:237-246); names cut to 12/40 | source ids from extras or metadata, else the lowest free positional id (psse:1440-1454; fmt:1375-1394); names sanitized with `value_substituted` (psse:1403-1411) | powerio does more | none |
| non-finite values | per-field substitutes 1.0, 0.0, 9999 (PC/AbstractConverter.java:548-562; PC/GeneratorConverter.java:141-155) | sentinel 1e10 with `not_a_number` (psse:272-295, 1397-1402) | no gap | none |

### 2.3 powerio PSS/E diagnostic emission sites

Code constants: the `EMIT.PSSE.*` family entries are built at diag:58-144 and instantiated at diag:173; `READ_PSSE_*` at diag:197-208; `EMIT_PSSE_DOWNGRADED` diag:375-376; `EMIT_PSSE_RATING_SET_REMAPPED` diag:377-378; both the `Psse` and `PsseRawx` targets use `EMIT_PSSE` (diag:415). `F` in psse.rs is `EMIT_PSSE` (psse:29).

RAW writer (psse):

| site | code | message literal | condition |
|---|---|---|---|
| 101-109 (text 121-136) | EMIT_PSSE_RATING_SET_REMAPPED | `"branch {n} ({from} to {to}) rating set {name}={mva} MVA emitted as {RATEk} in PSS/E v34/v35; rating set names outside RATE4-RATE12 are not preserved"` | rev >= 34, a rating set not named RATE4..RATE12 (or whose slot is taken) placed in a free slot |
| 111-115 (text fmt:1511-1526) | F.rating_set_dropped | `"... rating set {name}={mva} MVA dropped: PSS/E v34/v35 has no field for branch rating sets beyond rate_a, rate_b, and rate_c"` | rev >= 34, more rating sets than free RATE4..12 slots |
| 138-140, called 1354-1356 (fmt:1584-1598) | F.rating_set_dropped | same text with target "PSS/E v33" | rev 33, every rating set |
| 168-173, called 1262 | F.field_dropped | `"{total} generator energy source value(s) dropped ({summary}): PSS/E RAW and RAWX generator records have no energy source field"` | any generator energy_source other than Other |
| 398-406 | F.record_dropped | `"PSS/E load at bus {bus} id {id:?}: load type requires revision 35; dropped"` | rev < 35 and a typed load_type |
| 482-490 | F.field_dropped | `"PSS/E generator at bus {bus} id {id:?}: regulating node {node} has no NREG field before revision 35; emitted only IREG"` | rev < 35 and resolved node != 0 |
| 525-533 | F.field_dropped | `"PSS/E generator at bus {bus} id {id:?}: BASLOD {baslod} has no field before revision 35; dropped"` | rev < 35 and baslod != 0 |
| 701-709 | F.record_dropped | `"{n} system switching device record(s) dropped: PSS/E revision {rev} has no system switching device section"` | rev < 35, detailed connectivity included, switches nonempty |
| 754-762 | F.field_dropped | `"PSS/E transformer {from}-{to} ZCOD {zcod} dropped: the field requires revision 35"` | rev < 35, `psse_zcod` != 0 |
| 911-919 | F.field_dropped | `"PSS/E three winding transformer {i}-{j}-{k} ZCOD {zcod} dropped: the field requires revision 35"` | same for 3W |
| 963-972 | F.field_dropped | `"PSS/E three winding transformer winding at bus {bus}: COD 4 DC line quantity control is valid only for two winding transformers; emitted fixed control"` | 3W winding control mode DcLineQuantity |
| 1207-1214 | F.field_dropped | `"detailed connectivity dropped from PSS/E revision {rev} output: {error}"` | `write_raw_substation_data` returned an error |
| 1230-1235 | F.record_dropped | `"detailed substation connectivity dropped: PSS/E revision 33 has no substation data block"` | rev < 34 with substation records |
| 1239-1246 | F.value_defaulted | `"DC line converter detail (firing angles, converter transformer taps, reactive output) defaulted: PSS/E two-terminal DC is written from the power setpoint and line resistance only"` | any `Hvdc` where `dc_states_beyond_record` (1465-1509) is true |
| 1247-1255 | F.record_dropped | `"{n} storage unit(s) dropped: PSS/E has no storage record"` | storage nonempty |
| 1256-1261 | F.field_dropped | `"generator cost curves dropped: PSS/E .raw has no cost data"` | any generator cost |
| 1263-1268 | F.field_dropped | `"DC line cost curves dropped: PSS/E .raw has no cost data"` | any hvdc cost |
| 1269-1274 | F.field_dropped | `"branch angle limits (angmin/angmax) dropped: PSS/E branch records carry none"` | any `has_angle_limits` |
| 1287-1294 | F.field_dropped | `"{n} non-transformer branch name(s) dropped: PSS/E revision 33 branch records have no name field"` | rev 33 and named lines |
| 1301-1305 | F.field_dropped | `"{n} branch current rating record(s) dropped: PSS/E branch ratings are MVA ratings"` | any `current_ratings` |
| 1312-1319 | F.field_dropped | `"{n} switch current rating(s) dropped: PSS/E system switching device records carry MVA ratings, not current ratings"` | rev >= 35, switch current_rating |
| 1330-1337 | F.field_dropped | `"{n} switch power flow result set(s) dropped: PSS/E system switching device records carry no power flow result fields"` | rev >= 35, switch pf/qf/pt/qt |
| 1344-1351 | F.field_dropped | `"{n} system switching device rating set name(s) dropped: PSS/E RAW revision 35 writes explicit RATE1-RATE12 fields"` | rev >= 35, `psse_rsetnam` extras |
| 1360-1366 (fmt:1534-1563) | F.extras_dropped | `"{n} element(s) carry source-format passthrough fields (extras) the PSS/E .raw writer does not replay; dropped"` | any extras key other than `id` or `psse_*` |
| 1372-1376 | F.field_dropped | `"{n} branch solution value set(s) dropped: PSS/E RAW power flow result fields are not written"` | any branch solution |
| 1386-1390 | F.value_collapsed | `"{n} transformer terminal admittance record(s) collapsed to magnetizing admittance: PSS/E transformer records cannot preserve terminal side assignment"` | a transformer with nonzero g_to or b_to |
| 1391-1396 | F.field_dropped | `"generator ramp/capability columns dropped: PSS/E .raw has no equivalent fields"` | any `has_caps` |
| 1397-1402 | F.not_a_number | `"non-finite values written as +/-1e10 sentinels (PSS/E has no Inf/NaN)"` (the literal uses the plus-minus character) | any non-finite value formatted (272-295) |
| 1403-1411 | F.value_substituted | `"{n} quoted PSS/E field(s) contained a quote or '/' that would corrupt a record; replaced with spaces"` | any sanitized quoted field |
| 1553-1560 | F.value_substituted | `"PSS/E generator at bus {bus}: retained {PROP} value {raw:?} is not a finite number; emitted default {default}"` | generator metadata float not finite |
| 1582-1589 | F.value_substituted | `"PSS/E generator at bus {bus}: retained {PROP} value {raw:?} is not a finite integer code; emitted default {default}"` | generator metadata integer invalid |
| 1620-1625 | F.field_dropped | `"PSS/E {description}: the regulating terminal has no PSS/E bus and node mapping; emitted the regulated bus with node 0"` | a regulating terminal set but not resolvable through detailed connectivity |
| 1646-1652 | F.field_dropped | `"PSS/E {description}: a negative CONT requires a nonzero controlled bus; emitted CONT=0"` | `controlled_bus_on_winding_side` with no controlled bus |
| 4358-4361 | F.field_dropped | `"PSS/E load at bus {bus} id {id:?}: nominal voltage has no load record field; dropped"` | Zip model with v_nom |
| 4372-4379 | F.value_substituted | `"PSS/E load at bus {bus} id {id:?}: stale voltage model components did not match typed p/q; wrote typed p/q as constant power"` | Zip components do not sum to p/q |
| 4383-4386 | F.field_dropped | `"PSS/E load at bus {bus} id {id:?}: exponential voltage model has no load record fields; wrote typed p/q as constant power"` | Exponential model |
| 4404-4411 | F.value_substituted | `"PSS/E load at bus {bus} id {id:?}: stale PL/QL/IP/IQ/YP/YQ extras did not match typed p/q; wrote typed p/q as constant power"` | extras components do not sum to p/q |
| 4307-4310, 4315-4318 | READ_PSSE_VALUE_SUBSTITUTED (a read code emitted on the write path) | `"PSS/E load at bus {bus} id {id:?}: non-finite typed scaling has no SCAL value; used source/default SCAL"` and `"...: non-integer typed scaling {scaling} has no SCAL value; used source/default SCAL"` | typed `scaling` not a finite integer |

RAWX writer (rawx; also used by the RAW writer for substation records through rawx:2984-3083):

| site | code | message literal | condition |
|---|---|---|---|
| 1894-1899 | EMIT_PSSE.field_dropped | `"solver parameters dropped: PSS/E RAWX 35 has no system wide table"` | `solver` present |
| 2156-2161 | EMIT_PSSE.field_dropped | `"{count} system switching device rating set name(s) dropped: one RAWX `sysswd` table cannot mix `rsetnam` rows with explicit RATE1-RATE12 rows"` | switches with both `psse_rsetnam` and numeric ratings |
| 2418-2426 | EMIT_PSSE.record_dropped | `"{count} detailed {record} record(s) dropped: PSS/E RAWX substation tables cannot represent them"` | each nonempty list among subnetwork, bus breaker bus, calculated bus, junction, internal connection, operational limit group, tap changer, equipment reactive limit, dangling line, tie line, DC converter unit, DC topological node, DC node, DC ground, DC busbar, DC line detail, DC series device, DC switch, VSC detail, LCC detail |
| 2429-2437 | EMIT_PSSE.field_dropped | `"{n} detailed source omission marker(s) dropped: PSS/E RAWX cannot retain source absence markers"` | `omitted_fields` nonempty |
| 2448-2455 | EMIT_PSSE.field_dropped | `"{n} substation country, operator, or geographical tag value(s) dropped: PSS/E RAWX `sub` has no corresponding fields"` | any such substation field |
| 2472-2479 | EMIT_PSSE.extras_dropped | `"{n} detailed component metadata value(s) dropped: PSS/E RAWX emits only its named substation, node, switch, and terminal fields"` | metadata beyond the emitted `psse_*` set (2575-2614) |
| 2486-2493 | EMIT_PSSE.field_dropped | `"{n} topology switch retained flag(s) dropped: PSS/E RAWX switching device records have no retained field"` | retained switches |
| 2499-2506 | EMIT_PSSE.value_collapsed | `"{n} load break switch kind(s) emitted as disconnectors: PSS/E RAWX has no distinct load break switch code"` | LoadBreakSwitch kinds |
| 2564-2571 | EMIT_PSSE.field_dropped | `"{count} {description} dropped: PSS/E RAWX `subterm` has no corresponding field"` | terminal component identities, bus and connectable bus references, disconnected terminal flags, terminal active and reactive power values, noncanonical terminal sequence numbers |
| 2632-2639 | EMIT_PSSE.record_dropped | `"{n} detailed substation, voltage level, busbar, switch, or terminal record(s) dropped: PSS/E RAWX substation tables require connectivity nodes"` | detailed connectivity without nodes |
| 2858-2865 | EMIT_PSSE.record_dropped | `"{n} detailed topology switch record(s) dropped: PSS/E RAWX `subswd` requires two connectivity node endpoints"` | a switch with a bus endpoint |
| 2965-2972 | EMIT_PSSE.record_dropped | `"{n} detailed terminal record(s) dropped: their equipment type has no PSS/E RAWX `subterm.type` code"` | a terminal on unmappable equipment (2355-2378) |
| 3074-3081 | EMIT_PSSE.value_substituted | `"{n} quoted PSS/E substation field(s) (...) contained a quote or line break; replaced with spaces"` | RAW substation output sanitized a string |

Shared passes applied to the PSS/E targets (fmt, called from 1127-1131): `EMIT_PSSE_DOWNGRADED` (fmt:1403-1427, called 1049), `reference_missing` (fmt:1615-1631), `element_relabeled` (fmt:1655-1662), locations (fmt:1460-1480); the frequency pass exempts PSS/E and RAWX (fmt:1433-1439); an unsupported revision is an `Error::Emit` "unsupported revision {rev}; emission supports only revisions 33, 34, and 35" (fmt:1084-1096).

RAW reader (psse):

| site | code | message literal | condition |
|---|---|---|---|
| 1816-1823 | READ_PSSE_REFERENCE_DROPPED | `"{description}: node {node} at regulated bus {bus} has no detailed connectivity terminal"` | NREG or NODE nonzero without a matching terminal (RAW 35 path) |
| 2273-2276 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E transformer {label} pair {pair}: CZ=2 impedance has invalid SBASE {sbase}; read as system-base p.u."` | CZ 2 with a nonpositive SBASE |
| 2281-2284 | READ_PSSE_VALUE_SUBSTITUTED | `"... CZ=3 impedance has invalid SBASE {sbase}; read as system-base p.u."` | CZ 3 with a nonpositive SBASE |
| 2296-2299 | READ_PSSE_VALUE_UNSUPPORTED | `"... unsupported CZ={other}; read impedance as system-base p.u."` | CZ not 1..3 |
| 2334-2337 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E transformer {label}: CM=2 magnetizing data needs positive system, winding, bus-voltage, and nominal-voltage bases; read MAG1/MAG2 as p.u. admittance"` | CM 2 with a bad base |
| 2347-2350 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E transformer {label}: CM=2 exciting current magnitude {y} is below conductance {g}; set magnetizing susceptance to 0"` | CM 2, y^2 < g^2 |
| 2354-2357 | READ_PSSE_VALUE_UNSUPPORTED | `"PSS/E transformer {label}: unsupported CM={other}; read MAG1/MAG2 as p.u. admittance"` | CM not 1..2 |
| 2413-2416 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E transformer {label} {winding}: CW={cw} needs a positive bus base kV for bus {bus}; read {field} as a p.u. tap ratio"` | CW 2/3 with base_kv <= 0 |
| 2430-2433 | READ_PSSE_VALUE_UNSUPPORTED | `"PSS/E transformer {label} {winding}: unsupported CW={other}; read {field} as a p.u. tap ratio"` | CW not 1..3 |
| 2453-2456 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E transformer {label}: winding 2 ratio is zero; used winding 1 ratio as the branch tap"` | WINDV2 ratio 0 |
| 2500-2510 | READ_PSSE_SECTION_UNSUPPORTED | `"PSS/E {name} section ({rows} record line(s)) is not modeled: preserved only in a same-format .raw ..., dropped on any other write"` | each skipped section with records (SUBSTATION excluded at rev >= 34, 2095-2097) |
| 2520-2525 | READ_PSSE_REFERENCE_DROPPED | `"PSS/E GENERATOR DATA record {n} at bus {bus}: IREG references missing bus id {id}; dropped remote voltage control"` | IREG bus absent |
| 2536-2541 | READ_PSSE_REFERENCE_DROPPED | `"PSS/E TRANSFORMER DATA record {n} ({from}-{to}): CONT references missing bus id {id}; dropped transformer control pointer"` | 2W CONT bus absent |
| 2555-2564 | READ_PSSE_REFERENCE_DROPPED | `"PSS/E TRANSFORMER DATA record {n} winding {w} at bus {bus}: CONT references missing bus id {id}; dropped transformer control pointer"` | 3W CONT bus absent |
| 2577-2582 | READ_PSSE_REFERENCE_DROPPED | `"PSS/E SWITCHED SHUNT DATA record {n} at bus {bus}: SWREM references missing bus id {id}; dropped switched shunt control pointer"` | SWREM/SWREG bus absent |
| 2590-2595 | READ_PSSE_REFERENCE_DROPPED | `"PSS/E AREA DATA record {n} area {a}: ISW references missing bus id {id}; dropped area swing pointer"` | ISW bus absent |
| 2623-2629 | READ_PSSE_FIELD_DROPPED | `"PSS/E system wide {keyword} record has no typed representation; retained only in the same format source"` | GAUSS, ADJUST, TYSL, RATING lines |
| 2636-2642 | READ_PSSE_FIELD_DROPPED | `"PSS/E system wide {keyword} token {tok:?} is not a KEY=VALUE field; retained only in the same format source"` | a token without "=" |
| 2680-2685 | READ_PSSE_FIELD_DROPPED | `"PSS/E system wide {keyword}.{key} has no typed representation; retained only in the same format source"` | an unknown key on GENERAL/NEWTON/SOLVER |
| 2697-2702 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E system wide {keyword}.{key} value {value:?} is not {expected}; left the typed solver field unset"` | an unparseable THRSHZ, TOLN, ITMXN, or flag |
| 3166-3169 | READ_PSSE_RETAINED_SOURCE_ONLY | `"PSS/E load at bus {bus} id {id:?}: interruptible/DG/flag fields are retained in extras"` | INTRPT, PDGEN, QDGEN, or FLAGSTATUS non-default |
| 3223-3230 | READ_PSSE_FIELD_DROPPED | `"PSS/E switched shunt at bus {bus} block {k} has S={status}; block status is not represented and the block was retained as enabled"` | rev >= 35, block S != 1 |
| 3825-3830 | READ_PSSE_VALUE_UNSUPPORTED | `"PSS/E transformer {label} {winding}: unsupported COD={cod}; read its remaining fields with fixed control"` | abs(COD) outside 0..5 |
| 4122-4126 | READ_PSSE_VALUE_SUBSTITUTED | `"two-terminal DC record {n} states a drop model that does not evaluate to a finite power; both ends read as zero"` | non-finite pf/pt |
| 4133-4140 | READ_PSSE_VALUE_SUBSTITUTED | `"two-terminal DC record {n} schedules a current with no scheduled voltage; the demand cannot be priced into power and both ends read as zero"` | MDC 2 with VSCHD <= 0 |

Rev >= 34 RAW sources also pass through the RAWX detailed connectivity reader (psse:2122-2124 calling rawx:1669-1854, then rawx:1265-1664), so the RAWX reader sites at 250-257, 1290-1293, and 1560-1566 apply to RAW as well.

RAWX reader (rawx):

| site | code | message literal | condition |
|---|---|---|---|
| 250-257 | READ_PSSE_FIELD_DROPPED | `"PSS/E RAWX table `{table}` fields {list} are not modeled; retained only in the same format source"` | an unknown column in a known table |
| 322-328 | READ_PSSE_FIELD_DROPPED | `"PSS/E RAWX `caseid.{field}` value {value} is not retained in the balanced network; retained only in the same format source"` | ic != 0, xfrrat != 0, nxfrat != 1 |
| 333-336 | READ_PSSE_FIELD_DROPPED | `"PSS/E RAWX `caseid.title2` is not retained in the balanced network; retained only in the same format source"` | nonempty title2 |
| 451-455 | READ_PSSE_VALUE_SUBSTITUTED | `"{n} RAWX string value(s) contained an apostrophe, newline, or carriage return that the shared PSS/E record reader cannot carry; replaced with spaces"` | any sanitized string |
| 695-701 | READ_PSSE_FIELD_DROPPED | `"PSS/E RAWX `caseid` fields {list} are not modeled; retained only in the same format source"` | an unknown caseid field |
| 710-716 | READ_PSSE_FIELD_DROPPED | `"PSS/E RAWX `caseid` object members {list} are not modeled; retained only in the same format source"` | a caseid member other than fields/data |
| 731-743 | READ_PSSE_SECTION_UNSUPPORTED | `"PSS/E RAWX network member `{name}` is not modeled; retained only in the same format source"` or `"PSS/E RAWX `{name}` table contains {count} unmodeled record(s); retained only in the same format source"` | an unknown network member with content |
| 767-773 | READ_PSSE_SECTION_UNSUPPORTED | `"PSS/E RAWX `{name}` table contains {n} {description} record(s); retained only in a same format source ..."` | vscdc, impcor, ntermdc, ntermdcconv, ntermdcbus, ntermdclink, msline, zone, iatransfer, owner, facts, gne, indmach with rows |
| 822-830 | READ_PSSE_RETAINED_SOURCE_ONLY | `"PSS/E RAWX system switching device {n} ({from} to {to}) carries XPU in extras; the typed switch model has no impedance field"` | sysswd xpu != 0 |
| 1145-1150 | READ_PSSE_REFERENCE_DROPPED | `"{description}: node {node} at regulated bus {bus} has no detailed connectivity terminal"` | nreg or node nonzero without a terminal |
| 1290-1293 | READ_PSSE_VALUE_SUBSTITUTED | `"PSS/E RAWX `subswd` declared `rsetnam` as its final field but supplied three numeric values after `xpu`; read them as `rate1`, `rate2`, and `rate3`"` | PowSybl-shaped subswd rows (181-198) |
| 1560-1566 | READ_PSSE_RETAINED_SOURCE_ONLY | `"PSS/E RAWX terminal refers to equipment `{key}` outside the typed balanced calculation view"` | subterm equipment not in the typed tables |

### 2.4 PSS/E fixes in priority order

1. VSC DC lines: PowSybl imports and exports them; powerio warns on read and silently writes any VSC `Hvdc` (from XIIDM or CGMES) as an LCC two-terminal record (psse:1076-1124). The model already has `HvdcConverterKind::Vsc` (net:3104-3128). Large.
2. FACTS STATCOM: PowSybl maps it to a `StaticVarCompensator` both ways; powerio has the type (net:2296-2314) but the PSS/E reader and writer never touch it, and the writer drops SVCs without a diagnostic. Medium.
3. Three-winding STAT 2/3/4 collapses to in service (psse:4026, 932). Medium.
4. Switched shunt per-block status S1..S8 is warned and discarded (psse:3221-3231); PowSybl uses it. Medium.
5. Generator `voltage_regulation_on` is always true (psse:3477); PowSybl derives it from the bus IDE and the Q limits. Small.
6. Two-terminal DC: `Hvdc.resistance_ohm` and `nominal_voltage_kv` are left None although RDC and VSCHD are read (psse:4193-4194), and the writer ignores typed `HvdcConverter` fields. Small plus medium.
7. `typed_psse_scal` (psse:4307-4318) emits a `READ_PSSE_*` code from the write path; it should use `F.value_substituted`. Small.
8. No update mode: PowSybl's default export patches the imported model and keeps every unmodeled section; powerio regenerates after any edit and drops them, with warnings only at read time. Large.
9. The RAW header IC is ignored silently (psse:1869-1908) while RAWX warns (rawx:319-329); PowSybl refuses IC = 1. Small.
10. Storage: PowSybl writes batteries as negative loads (PC/BatteryConverter.java:37-56); powerio drops storage with a warning. Small.

Two stale PowSybl notes for the worker: PD/import.md:66 says the constant current and admittance load components are discarded, but PC/LoadConverter.java:47-81 builds a `ZipModel`; PC/LineConverter.java:109-111 warns "Branch G not supported" although GI/GJ are mapped at 64-69.

<!-- XIIDM-SECTION -->

## 3. XIIDM

See `powsybl-gaps-xiidm.md`: two defects (SlackTerminal never read; VSC capability curve collapsed on export), the small and medium fixes, and the reader diagnostics that already name a format limitation.

## 4. CGMES

Aliases for this section:

| alias | path |
|---|---|
| PS | /home/sam/Research/powsybl-core/cgmes/cgmes-conversion/src/main/java/com/powsybl/cgmes/conversion |
| CD | /home/sam/Research/powsybl-core/docs/grid_exchange_formats/cgmes |
| RD, WR, MOD, XML | /home/sam/Research/powerio/powerio-tx/src/format/cgmes/{read,write,mod,xml}.rs |

Codes: RU = `READ.CGMES.RECORD_UNMAPPED`, FU = `READ.CGMES.FIELD_UNMAPPED`, VD = `READ.CGMES.VALUE_DEFAULTED`, VA = `READ.CGMES.VALUE_APPROXIMATED` (diag:223-230); `EMIT_CGMES.*` is the writer family (diag:182). Every bare `push(` in the reader resolves to RU (collections created at RD:840 and RD:3343); every bare `push(` in the writer resolves to `EMIT_CGMES.record_dropped` (WR:3817). "FormatRead" is `Error::FormatRead { format: "CGMES" }`, code `PARSE.SOURCE.MALFORMED`.

Three corrections to the premises of the task, found while reading PowSybl:

1. This PowSybl checkout does not map `TopologicalIsland.AngleRefTopologicalNode` to `SlackTerminal` on import; `AngleRef` and `SlackTerminal` appear only in `PS/export/StateVariablesExport.java`. On export the angle reference comes from the `ReferenceTerminals` extension (PS/export/StateVariablesExport.java:114-151), and `SlackTerminal` drives the slack `SvInjection` (354-382) and the load flow status exemption (269-272).
2. CD/import.md:458 says CGMES `Switch` and `ProtectedSwitch` become BREAKER; PS/elements/SwitchConversion.java:114-122 maps them to DISCONNECTOR.
3. CD/import.md:865 says `disconnect-boundary-line-if-boundary-side-is-disconnected` defaults to false; PS/CgmesImport.java:796 and PS/Conversion.java:1128 default it to true.

Fix size on a "no gap" or "powerio does more" row names the size of the change that would align powerio with PowSybl if that were wanted.

### 4.1 CGMES import

| element or attribute | PowSybl (file:line) | powerio (file:line) | gap kind | fix size |
|---|---|---|---|---|
| **Topology, buses, slack** | | | | |
| node breaker versus bus branch decision | PS/Context.java:51 nodeBreaker = isNodeBreaker and not import-node-breaker-as-bus-breaker; PS/Conversion.java:191-196; option PS/CgmesImport.java:787-791 | RD:1118-1158 calculation buses always from TopologicalNode, error when none ("node-breaker collapse is follow-up work" RD:1154-1156); per VoltageLevel topology kind from ConnectivityNode presence RD:1712-1716 | powerio defaults it | large |
| EQ only set (no TP) | PS/Conversion.java:150-152 requires EquipmentCore only | RD:936-947 FormatRead when EQ or TP is missing | powerio drops it | large |
| TopologicalNode to bus (id, name) | PS/elements/NodeConversion.java:147-152 | RD:1144-1148 | no gap | small |
| TopologicalNode.BaseVoltage nominalVoltage | PS/elements/VoltageLevelConversion.java:53-63 (NaN throws, zero ignored 34-40) | RD:1120-1143 required; missing or nonpositive is FormatRead | PowSybl warns too | small |
| TopologicalIsland.AngleRefTopologicalNode to slack | not imported (correction 1) | RD:1311-1322 the first island's angle reference bus becomes `BusType::Ref` | powerio does more | small |
| slack when no angle reference is stated | none (IIDM has no bus type) | RD:1307-1328 lowest positive `SynchronousMachine.referencePriority` (RD:3873-3877), else the first ExternalNetworkInjection bus (RD:3905-3907), else the largest ratedS machine (RD:3878-3880), else RU; norm:819-833 recomputes Ref/Pv/Pq and norm:635-662 promotes the largest pmax generator bus | powerio does more | small |
| bus types PV, PQ, isolated | none | RD:1144 all Pq, RD:1329-1334 Pv when a generator sits on the bus; isolated never assigned (norm:781-783 drops it) | powerio does more | small |
| ConnectivityNode to node, internal connections | PS/elements/NodeConversion.java:129-145; PS/NodeMapping.java:47-118 one IIDM node per terminal plus internal connection | RD:1723-1738 ConnectivityNode record; RD:1891-1895 `Terminal.node` is the CN itself; RD:2110 `internal_connections` empty | no gap | small |
| Terminal.sequenceNumber | PS/elements/AbstractConductingEquipmentConversion.java:714-723 aliases CGMES.Terminal1..3 | RD:683-686, 1871-1875 default 1 | no gap | small |
| Terminal SSH ACDCTerminal.connected | PS/elements/AbstractConductingEquipmentConversion.java:531-556 (bus breaker only; node breaker uses fictitious switches) | RD:701-703, 1896 kept in both kinds; RD:727-733 energized | powerio does more | small |
| disconnected terminal in node breaker | fictitious open Breaker `<terminalId>_SW_fict` PS/elements/TerminalConversion.java:132-164; option PS/CgmesImport.java:764-769 | RD:1896 `Terminal.connected` retained, no fictitious switch | no gap | small |
| Terminal.TopologicalNode (TP) | PS/TerminalMapping.java:132-138 | RD:694-700 direct or via CN.TopologicalNode | no gap | small |
| Terminal.phases, TopologicalNode.ReportingGroup | not read | FU grouped RD:4905-4911 | no gap | small |
| dangling Terminal or other required reference | boundary or missing, element skipped PS/elements/AbstractConductingEquipmentConversion.java:127-152, 186-188 | FormatRead for any dangling reference among 33 required properties RD:960-1022 | PowSybl warns too | medium |
| SvVoltage v, angle | copied only when v > 0 and angle finite, else NaN plus `invalidAngleVoltageBus` report PS/elements/NodeConversion.java:154-172, 205-214 | RD:1163-1188 copied as is (vm = v / base_kv); conflicting observations FormatRead RD:1168-1177 | PowSybl warns too | small |
| SvVoltage or SvPowerFlow from another modelingAuthoritySet | each IGM becomes a subnetwork PS/CgmesImport.java:177-188, 263-446 | RD:1189-1216 RU, property RD:2050-2057; flow not mapped RD:1224-1232 | powerio warns where PowSybl carries it | large |
| SvPowerFlow p, q per terminal | PS/elements/AbstractConductingEquipmentConversion.java:540-547 | RD:1217-1254 (partial observation RU, conflict FormatRead), stored RD:1877, 1897-1898; SSH fallback RD:766-790 | no gap | small |
| SvStatus.inService | not read on import | RD:1255-1287 (missing reference or non boolean FormatRead); precedence over SSH RD:812-831; property RD:2044-2049 | powerio does more | small |
| SvInjection | fictitious Load PS/Update.java:232-238, PS/elements/SvInjectionConversion.java:52-87; option PS/CgmesImport.java:719-723 | not in CONSUMED RD:1045-1101; class count RU RD:4893-4896 | powerio drops it | medium |
| **Switches** | | | | |
| switch classes | PS/elements/SwitchConversion.java:114-122 (Disconnector, GroundDisconnector, Jumper to DISCONNECTOR; LoadBreakSwitch; Breaker; default including Switch and ProtectedSwitch to DISCONNECTOR) | RD:1025-1034 Breaker, Disconnector, LoadBreakSwitch, Switch, Fuse, Jumper, GroundDisconnector, DisconnectingCircuitBreaker (no ProtectedSwitch, Cut, Sectionaliser, Recloser); kind RD:1933-1937 (Jumper, Switch, Fuse become Breaker) | powerio defaults it | small |
| Switch.normalOpen, SSH open | PS/elements/SwitchConversion.java:81, 97-98, 145-152 | RD:1940-1943, 4223-4226 | no gap | small |
| Switch.retained | PS/elements/SwitchConversion.java:87-88 node breaker only | RD:1944 | no gap | small |
| Switch.ratedCurrent | not read | RD:4228 current_rating | powerio does more | small |
| Switch.switchOnCount, switchOnDate, locked | not read | FU grouped RD:4905-4911 | no gap | small |
| switch with both ends on one bus (bus branch) | ignored with a warning (silenced by option) PS/elements/SwitchConversion.java:43-48 | RD:4216-4221, 4233-4241 counted, one VA; TopologySwitch kept RD:1902-1947 | PowSybl warns too | small |
| switch between two VoltageLevels or Substations | merged to the alphabetically first with LOG.warn PS/NodeContainerMapping.java:168-191, 265-295 | RD:1909-1911 switch VL = the first terminal's VL, levels kept distinct | powerio does more | small |
| switch with one end at a boundary node | zero impedance dangling line PS/elements/SwitchConversion.java:102-112 | RD:4210-4214 skipped; a detailed switch without topology FormatRead RD:1920-1925 | powerio drops it | large |
| **Hierarchy and regions** | | | | |
| GeographicalRegion, SubGeographicalRegion (country from regionName then subRegionName, geographicalTags) | PS/elements/SubstationConversion.java:38-43, 58-60, 71-73; PS/CountryConversion.java:25-60 | classes not consumed RD:4816-4839; `Substation.country` only from `entsoe:Substation.Country` RD:1675-1677, tags empty RD:1679 | powerio drops it | medium |
| Substation | PS/elements/SubstationConversion.java:31-34, 53-61 | RD:1670-1682 | no gap | small |
| substations joined by transformers merged | PS/NodeContainerMapping.java:217-227, 281-295 | none | powerio does more | small |
| VoltageLevel nominalVoltage zero | ignored plus `nominalVoltageIsZero` report PS/elements/VoltageLevelConversion.java:34-40 | RD:1687-1690 0.0 stored; the writer refuses it WR:3801-3811 | powerio defaults it | small |
| VoltageLevel.Substation absent | `missingMandatoryAttribute` report, VL skipped PS/elements/VoltageLevelConversion.java:41-47 | RD:1705-1708 Option | PowSybl warns too | small |
| VoltageLevel.lowVoltageLimit, highVoltageLimit | PS/elements/VoltageLevelConversion.java:55-56, 79-96 | RD:1710-1711, combined with VoltageLimit RD:3213-3229 | no gap | small |
| BaseVoltage identity (BaseVoltageMapping extension) | keyed by kV with IGM or BOUNDARY source PS/Conversion.java:175-177, 492-495 | value only; several ids for one kV VA RD:1519-1552 | powerio warns where PowSybl carries it | medium |
| ConductingEquipment.BaseVoltage consistency | not used | RD:1464-1517 checked against connected levels, FU and VA | powerio does more | small |
| Line container nodes | fictitious VL per node or container PS/Conversion.java:683-717; option PS/CgmesImport.java:815-820 | RD:1417 "line" container kept; the writer generates a VoltageLevel WR:4251-4261 | no gap | small |
| Substation container holding nodes | ConversionException PS/NodeContainerMapping.java:143-152 | RD:1725-1731 accepted; the writer `value_defaulted` WR:4319-4324 | powerio does more | small |
| Bay container | resolved through the hierarchy PS/elements/NodeConversion.java:114-115 | only VoltageLevel counts RD:1435-1439; container kept RD:2063-2071; the writer substitutes the terminal VL with `value_substituted` WR:379-418 | powerio warns where PowSybl carries it | small |
| Junction | not converted | RD:1854-1861 | powerio does more | small |
| Ground | PS/elements/GroundConversion.java:25-32; RemoveGrounds post processor PS/RemoveGroundsPostProcessor.java:45-50 | not consumed, RU class count RD:4893-4896 | powerio drops it | medium |
| BusbarSection | node breaker only PS/elements/BusbarSectionConversion.java:37-46 | RD:1786-1852 both kinds, synthesized CN in bus branch RD:1833-1842 | powerio does more | small |
| **Boundary set** | | | | |
| EQ_BD, TP_BD files, boundary-location, convert-boundary | PS/CgmesImport.java:104-110, 475-517, 713-718; PS/CgmesBoundary.java:29-72 | merged as ordinary documents RD:293-305, 894-906; boundary TopologicalNodes become ordinary buses RD:1118-1150 | powerio defaults it | large |
| boundary CN attributes (boundaryPoint, fromEndIsoCode, toEndIsoCode, fromEndName, toEndName, fromEndNameTso, toEndNameTso, EIC, DC flag) | PS/CgmesBoundary.java:51-82; country from iso codes only with convert-boundary PS/elements/NodeConversion.java:77-88 | FU grouped RD:4905-4911 | powerio drops it | large |
| EquivalentInjection at a boundary node | deferred and folded into the dangling line PS/elements/EquivalentInjectionConversion.java:32-47, 57-96; PS/elements/AbstractConductingEquipmentConversion.java:221-231, 430-449 (several in one graph invalid 443-447) | RD:4247-4286 load, or a limitless Generator when regulationStatus is true, one VA count | powerio drops it | large |
| ACLineSegment, EquivalentBranch, Switch, PowerTransformer with one end at a boundary node to a dangling line | PS/Conversion.java:662-793, 809-831; PS/elements/ACLineSegmentConversion.java:81-95; PS/elements/transformers/TwoWindingsTransformerConversion.java:104-133 | branch when both terminals land on buses, else RU skip RD:4293-4304, 4418-4424; the detailed dangling line list is empty RD:2114 (struct net:656-673) | powerio drops it | large |
| tie line pairing by boundary node name and region | PS/elements/TieLineConversion.java:39-58; three or more PS/Conversion.java:822-830 | `tie_lines` empty RD:2115 | powerio drops it | large |
| CgmesDanglingLineBoundaryNode (hvdc, line EIC) | PS/elements/AbstractConductingEquipmentConversion.java:357-372 | none | powerio drops it | large |
| disconnect-boundary-line-if-boundary-side-is-disconnected; several unpaired lines at one node | PS/Conversion.java:439-477 | none | powerio drops it | large |
| **Generators and injections** | | | | |
| GeneratingUnit.minOperatingP, maxOperatingP | PS/elements/SynchronousMachineConversion.java:45-46 defaults -MAX and MAX (0 for condensers); min > max swapped PS/elements/AbstractReactiveLimitsOwnerConversion.java:148-155 | RD:3868-3869 default 0.0, no swap | powerio defaults it | small |
| GeneratingUnit.initialP | property PS/elements/SynchronousMachineConversion.java:86-88; targetP fallback 134, 152-155 | property RD:2027-2032; pg never falls back to it (RD:762-810) | powerio defaults it | small |
| GeneratingUnit.normalPF (ActivePowerControl) | PS/elements/SynchronousMachineConversion.java:157-182, extension only when create-active-power-control-extension (PS/Conversion.java:1121) | RD:3784-3810 always typed; nonnumeric or negative is FormatRead RD:3791-3806 | powerio does more | small |
| RotatingMachine.ratedS | PS/elements/SynchronousMachineConversion.java:47-48 nonpositive becomes NaN | RD:3863 missing becomes 0.0 | no gap | small |
| RotatingMachine.ratedU, ratedPowerFactor | not queried | FU grouped | no gap | small |
| SynchronousMachine.type (isCondenser) | PS/elements/SynchronousMachineConversion.java:37-38, 57 drives the minP/maxP defaults | RD:2002-2016 text retained only; no condenser semantics | powerio defaults it | medium |
| SynchronousMachine.operatingMode (SSH) | queried, not applied PS/elements/SynchronousMachineConversion.java:126-150 | retained RD:2005-2015 | powerio does more | small |
| SSH RotatingMachine.p, q (sign) | targetP = -p, targetQ = -q PS/elements/SynchronousMachineConversion.java:136-141 | RD:3840-3843 pg = -p, qg = -q | no gap | small |
| SSH p, q source (SSH versus SV) | SSH only PS/elements/AbstractConductingEquipmentConversion.java:725-731; SV goes to terminal p, q 540-547 | RD:762-810 SSH, else SvPowerFlow (VA), else 0 (VD) | powerio does more | small |
| SynchronousMachine.referencePriority (also CGMES 3) | ReferencePriority extension when positive PS/elements/SynchronousMachineConversion.java:129-132 | RD:3873-3877 slack candidate; text RD:2017-2020 | no gap | small |
| ExternalNetworkInjection.referencePriority | extension PS/elements/ExternalNetworkInjectionConversion.java:65-68 | not typed, FU; the first ENI is only a slack fallback RD:3905-3912 | powerio warns where PowSybl carries it | small |
| RegulatingControl.mode (generator) | voltage or reactivePower, else ignored PS/RegulatingControlMappingForGenerators.java:85-91; reactivePower creates RemoteReactivePowerControl 116-144 | RD:3929-3936 voltage only, other modes RU and properties retained RD:1649-1654 | powerio warns where PowSybl carries it | large |
| RegulatingControl.targetValue missing or zero | replaced by the nominal V of the regulating terminal PS/elements/AbstractReactiveLimitsOwnerConversion.java:173, 235-240, report if still invalid 178-185 | RD:3953-3958 vg = target / kV only when target is positive, else vg stays 1.0 (net:2933), no diagnostic | no gap | small |
| RegulatingControl.targetValueUnitMultiplier | queried, unused (taken as kV) | RD:3975-3997 none, m, k, M, G scaling; unknown VD | powerio does more | small |
| RegulatingControl.enabled and RegulatingCondEq.controlEnabled | both required PS/elements/AbstractReactiveLimitsOwnerConversion.java:175-176, 185, 242-244 | RD:3938-3943 enabled default false, controlEnabled default true | powerio defaults it | small |
| RegulatingControl.discrete, targetDeadband (generators) | not read | retained RD:1639-1648 | powerio does more | small |
| RegulatingControl.Terminal (remote) | PS/RegulatingTerminalMapper.java:37-50 switch chain and busbar fallbacks; own terminal fallback PS/RegulatingControlMappingForGenerators.java:98-100 | RD:3944-3961 TerminalReference RD:2924-2941 plus regulated bus | no gap | small |
| RegulatingControl object absent | missing PS/RegulatingControlMappingForGenerators.java:80 | RD:3926-3936 silent (regulation off) | powerio defaults it | small |
| SynchronousMachine.qPercent (CoordinatedReactiveControl) | PS/RegulatingControlMappingForGenerators.java:50, 105-109 | FU | powerio warns where PowSybl carries it | medium |
| SynchronousMachine.minQ, maxQ | missing means unbounded plus a missing report (silenced) PS/elements/AbstractReactiveLimitsOwnerConversion.java:116-128; swapped when reversed 139-146 | RD:3844-3845 missing means 0.0, no swap, no warning | powerio defaults it | small |
| ReactiveCapabilityCurve, CurveData | curve wins; same p merged, NaN point ignored, one point becomes MinMax PS/elements/AbstractReactiveLimitsOwnerConversion.java:60-114, 130-137 | RD:2311-2378 curveStyle and W/VAr units required (RU otherwise); incomplete points dropped silently RD:2362-2369; duplicate p or fewer than two points FormatRead via net:570-584 at RD:3847-3855 | powerio warns where PowSybl carries it | small |
| WindGeneratingUnit.windGenUnitType, HydroPowerPlant, FossilFuel | properties PS/elements/SynchronousMachineConversion.java:90-104 | FU | powerio warns where PowSybl carries it | small |
| GeneratorEntsoeCategory | PS/EntsoeCategoryPostProcessor.java:58-83 parses the GeneratingUnit description | description retained RD:2026-2032 | no gap | small |
| generator cost | none in CGMES | cost None net:2936 | no gap | small |
| ExternalNetworkInjection minP, maxP, minQ, maxQ, governorSCD | defaults unbounded PS/elements/ExternalNetworkInjectionConversion.java:35-47; governorSCD property 56-59 | RD:3898-3901 default 0; governorSCD FU; VA that the class is represented as a balanced generator RD:3890-3895 | powerio defaults it | small |
| EquivalentInjection outside the boundary set (minP, maxP, minQ, maxQ, regulationCapability, regulationTarget, curve) | Generator with limits and regulation PS/elements/EquivalentInjectionConversion.java:98-136 | RD:4253-4275 Load, or Generator with pg, qg only; limits and target FU | powerio drops it | medium |
| **Loads** | | | | |
| EnergyConsumer, ConformLoad, NonConformLoad, StationSupply | PS/elements/EnergyConsumerConversion.java:26-60 (StationSupply becomes AUXILIARY) | RD:1037-1042; class in `cgmes_class` RD:1954-1959 | no gap | small |
| EnergyConsumer.pfixed, qfixed (EQ) | PS/elements/EnergyConsumerConversion.java:41-42, 142-158 fallback | FU; p, q from SSH, SvPowerFlow, else 0 RD:3773, 762-810 | powerio warns where PowSybl carries it | small |
| LoadDetail (conform and nonconform split) | PS/elements/EnergyConsumerConversion.java:120-137, 160-175 | class metadata only | powerio drops it | large |
| LoadResponseCharacteristic exponents | PS/elements/EnergyConsumerConversion.java:62-78 | RD:3696-3717 | no gap | small |
| LoadResponseCharacteristic ZIP coefficients | NaN skips silently, sums rescaled with a fixed report PS/elements/EnergyConsumerConversion.java:80-118 | RD:3661-3686, 3719-3759 missing returns None; sums normalized with VA, zero sum RU | no gap | small |
| pFrequencyExponent, qFrequencyExponent | not queried | FU | no gap | small |
| EnergySource, AsynchronousMachine | Load with load sign PS/elements/EnergySourceConversion.java:28-52, PS/elements/AsynchronousMachineConversion.java:31-56 | not consumed; the injection is lost, RU class count RD:4893-4896 | powerio drops it | medium |
| fictitious loads (id contains "fict") | LoadType.FICTITIOUS PS/elements/EnergyConsumerConversion.java:36-37 | `IdentifiedObject.isFictitious` into metadata RD:2086-2088 | no gap | small |
| **Shunts and SVC** | | | | |
| LinearShuntCompensator bPerSection, gPerSection | defaults Float.MIN_VALUE and NaN PS/elements/ShuntConversion.java:42-43 | RD:4021-4028 default 0, scaled by kV squared to MVAr | no gap | small |
| ShuntCompensator.maximumSections, normalSections | PS/elements/ShuntConversion.java:36-37, 85-87 | RD:4014-4018 default max(sections, 1); normalSections fallback RD:4191 | powerio defaults it | small |
| ShuntCompensator.nomU, aVRDelay, grounded | not used | FU | no gap | small |
| SSH sections | PS/elements/ShuntConversion.java:96-99 | RD:4177, 4189-4194 | no gap | small |
| SvShuntCompensatorSections (solvedSectionCount) | separate field PS/elements/ShuntConversion.java:88, 101-104 | fallback only; mismatch FU and the SV value dropped RD:4175-4195 | powerio drops it | medium |
| shunt RegulatingControl target, deadband, enabled, Terminal | regulation off unless targetV is positive and the deadband nonnegative PS/elements/ShuntConversion.java:110-139; terminal PS/RegulatingControlMappingForShuntCompensators.java:63-69 | RD:4126-4167 target and deadband default 0 (vhigh = vlow = 0); Discrete mode whenever enabled | powerio defaults it | small |
| NonlinearShuntCompensatorPoint | cumulative b, g PS/elements/ShuntConversion.java:49-62 | per point blocks RD:4057-4104, gap in numbering RU RD:4088-4097 | no gap | small |
| EquivalentShunt | one section linear shunt PS/elements/EquivalentShuntConversion.java:28-46 | not consumed | powerio drops it | medium |
| StaticVarCompensator inductiveRating, capacitiveRating | 0 or absent means unbounded with a fixed report PS/elements/StaticVarCompensatorConversion.java:35-40, 55-61 | 0 S with RU RD:2953-2977 | PowSybl warns too | small |
| StaticVarCompensator.slope (VoltagePerReactivePowerControl) | PS/elements/StaticVarCompensatorConversion.java:34, 48-50, 63-71 | FU; the writer emits 0.0 WR:5926 | powerio warns where PowSybl carries it | medium |
| sVCControlMode, voltageSetPoint, SSH q, controlEnabled | PS/RegulatingControlMappingForStaticVarCompensators.java:46, 88-99, 138-142; PS/elements/StaticVarCompensatorConversion.java:76-111 | RD:2980-3013 (any mode other than reactivePower becomes Voltage) | no gap | small |
| SVC RegulatingControl validity and terminal sign | PS/elements/StaticVarCompensatorConversion.java:81-101 | RD:2989-3018 no validity check, no sign | powerio defaults it | small |
| **Lines and series equipment** | | | | |
| ACLineSegment r, x, bch, gch | PS/elements/ACLineSegmentConversion.java:73-79, PS/elements/AbstractBranchConversion.java:79-86 half at each end | RD:4309-4320 per unit on the from bus kV, charging split in half | no gap | small |
| ACLineSegment length, r0, x0, b0ch, g0ch, shortCircuitEndTemperature | not read (CD/import.md:202-208) | FU grouped RD:4853-4912 | no gap | medium |
| ACLineSegment with r = x = 0 inside one VoltageLevel | fictitious retained BREAKER PS/elements/AbstractBranchConversion.java:51-77 | zero impedance Branch RD:4309 (the matrix build rejects it later) | powerio does more | small |
| EquivalentBranch r, x (r21, x21 rejected when different) | Line with g = b = 0 PS/elements/EquivalentBranchConversion.java:41-63 | not in CONSUMED RD:1045-1102; RU class count RD:4893-4896 | powerio drops it | small |
| SeriesCompensator r, x | PS/elements/SeriesCompensatorConversion.java:27-33 (switch when both ends are in one VL) | RD:4326-4349 always Branch; r0, x0, varistor fields retained RD:1963-1981 | powerio does more | small |
| **Two winding transformers** | | | | |
| PowerTransformerEnd r, x, g, b per end | PS/elements/transformers/CgmesT2xModel.java:44-52, 76-77 sums r, x; shunt by option (default END1_END2, PS/Conversion.java:1134) InterpretedT2xModel.java:113-139; structural ratio option X (PS/Conversion.java:1135) moved to end 1 ConvertedT2xModel.java:41-56 | RD:4428-4450 each end per unit on its own ratedU, summed; g, b per end in BranchCharging | no gap | small |
| PowerTransformerEnd ratedU | PS/elements/transformers/CgmesT2xModel.java:39-40; TwoWindingsTransformerConversion.java:146-149 | RD:4563-4584 fallback to bus kV with VD; folded into `Branch.tap` RD:4452-4467 | no gap | small |
| PowerTransformerEnd ratedS | first positive end PS/elements/transformers/CgmesT2xModel.java:56-65; 3W per leg CgmesT3xModel.java:52-53 | not read (a grep for ratedS in RD hits only `RotatingMachine.ratedS` at 3863); FU | powerio drops it | medium |
| PowerTransformerEnd endNumber | alias PS/elements/transformers/AbstractTransformerConversion.java:124-128 | RD:4366-4379; property RD:1993-1995 | no gap | small |
| PowerTransformerEnd phaseAngleClock | post processor: end 2 to the extension, end 1 ignored with LOG.warn PS/PhaseAngleClock.java:59-81 | FU | powerio drops it | medium |
| PowerTransformerEnd connectionKind, grounded, rground, xground, r0, x0, g0, b0 | not read PS/elements/transformers/CgmesT2xModel.java:37-52 | FU | no gap | medium |
| PowerTransformer with other than 2 or 3 ends | invalid PS/Conversion.java:773-777 | RU count RD:4386-4399 | PowSybl warns too | small |
| transformer without Substation | PowsyblException PS/elements/transformers/TwoWindingsTransformerConversion.java:136-138 | substation optional RD:1705-1708 | powerio does more | small |
| tap changers on both ends (combine, move end 2 to end 1) | PS/elements/transformers/InterpretedT2xModel.java:60-108; TapChangerConversion.java:59-132 (fixed position with context.fixed 137-140), 277-329 move with impedance corrections | balanced: end 1 factor multiplied, end 2 divided, shifts signed RD:4454-4466; detailed: one TapChanger per end RD:3589-3656 | powerio does more | small |
| **Tap changers** | | | | |
| lowStep, highStep, neutralStep, normalStep | PS/elements/transformers/AbstractCgmesTapChangerBuilder.java:49-66 (invalid neutral replaced by the closest ratio 1 step) | RD:3605-3631; high below low or above 10000 steps FormatRead RD:3527-3532 | no gap | small |
| stepVoltageIncrement, ltcFlag | PS/elements/transformers/CgmesRatioTapChangerBuilder.java:79-88; AbstractCgmesTapChangerBuilder.java:61-62 | RD:3538-3553, 3632-3637 | no gap | small |
| neutralU | not read | FU; the writer derives it WR:1700-1742, 1830 | no gap | small |
| tculControlMode | stored PS/elements/transformers/CgmesRatioTapChangerBuilder.java:35, re-exported for CIM16 PS/export/elements/TapChangerEq.java:88-91 | FU | powerio drops it | small |
| SSH step, SvTapStep | step else SvTapStep else normalStep, validity checked PS/elements/transformers/AbstractTransformerConversion.java:193-195, 326-362 | RD:3615-3621 step else normalStep else neutralStep else lowStep; solved from SvTapStep RD:4651-4657; balanced factor uses SSH step else SvTapStep RD:4634-4637 | no gap | small |
| TapChanger.controlEnabled, RegulatingControl.enabled, target validity | regulating only when target and deadband are valid and the terminal is mapped; bad values reported PS/elements/transformers/AbstractTransformerConversion.java:205-228, 263-301 | RD:3638-3643 controlEnabled else RegulatingControl.enabled; no validation | powerio does more | small |
| TapChangerControl mode | ratio: voltage only, other fixed PS/RegulatingControlMappingForTransformers.java:123-127; phase: currentflow, activepower 149-153 | RD:3401-3422 four modes, unknown VA | powerio does more | small |
| TapChangerControl targetValue, targetDeadband, Terminal | PS/elements/transformers/AbstractTransformerConversion.java:197-229, 263-301 (terminal sign for phase) | RD:3645-3653 raw | no gap | small |
| RegulatingControl.discrete, targetValueUnitMultiplier on tap controls | not read on import | properties RD:1640-1659 | powerio does more | small |
| RatioTapChangerTable, PhaseTapChangerTable and points | a table with a missing step falls back to linear (ignored) PS/elements/transformers/AbstractCgmesTapChangerBuilder.java:68-80; NaN point fixed 86-95; rho = 1 / ratio, alpha = -angle AbstractTransformerConversion.java:58, 94 | RD:3424-3458 sorted, missing values default silently, rho = ratio, alpha = angle | no gap | small |
| PhaseTapChangerLinear, Asymmetrical, Symmetrical (increments, windingConnectionAngle, xMin, xMax) | PS/elements/transformers/CgmesPhaseTapChangerBuilder.java:93-222, 240-256 | RD:3517-3584, 3460-3515 same formulas | no gap | small |
| unknown PhaseTapChanger subclass | invalid, dropped PS/elements/transformers/CgmesPhaseTapChangerBuilder.java:43-47 | RU, zero shift RD:4723-4729 | PowSybl warns too | small |
| **Three winding transformers** | | | | |
| per end r, x, g, b, ratedU, ratedS; ratedU0; leg model | PS/elements/transformers/CgmesT3xModel.java:33-54; ratedU0 by option STAR_BUS_SIDE = ratedU1 (InterpretedT3xModel.java:44-62, PS/Conversion.java:1139); ConvertedT3xModel.java:48-93; ThreeWindingsTransformerConversion.java:100-140 | RD:4485-4561 `Transformer3W`: star r, x per end on its ratedU, pairwise sums, magnetizing g, b summed, `Winding.tap` and `shift`; no ratedU0 (net:3343-3357) | no gap | medium |
| star bus voltage and angle | computed and stored as properties PS/elements/transformers/ThreeWindingsTransformerConversion.java:77-98 | net:3394-3396 defaults 1.0 and 0.0 | powerio defaults it | medium |
| **Operational limits** | | | | |
| OperationalLimitSet on a Terminal | group per side keyed by set id, name and id as properties PS/elements/OperationalLimitConversion.java:46-51, 91-126 | RD:3345-3349, 3383-3395 group per (equipment, terminal); id = set mRID; name not retained | no gap | small |
| OperationalLimitSet on Equipment (Line) | two groups, side 2 id suffixed "-1" PS/elements/OperationalLimitConversion.java:207-210 | one group per terminal with the same id RD:3350-3365 | no gap | small |
| OperationalLimitSet on Equipment (2W or 3W transformer) | ignored PS/elements/OperationalLimitConversion.java:211-227 | group per terminal RD:3350-3365 | powerio does more | small |
| limits on switches, generators, loads, boundary side | notAssigned, pending (silenced) PS/elements/OperationalLimitConversion.java:196-201, 471-492 | group for any class RD:3383-3395; balanced apply only to branches RD:4737-4752 | powerio does more | small |
| OperationalLimit value | normalValue then value; NaN or nonpositive ignored PS/elements/OperationalLimitConversion.java:250-257, 271-273 | RD:3254-3263 same order, RU; balanced RD:4790-4793 value then normalValue, default 0 | no gap | small |
| PATL detection | limitType patl or name PATL PS/elements/OperationalLimitConversion.java:332-336 | isInfiniteDuration else kind == patl RD:3265-3268 | no gap | small |
| TATL acceptableDuration absent | Integer.MAX_VALUE PS/elements/OperationalLimitConversion.java:416 | 0 seconds silently RD:3294-3295 | powerio defaults it | small |
| acceptableDuration fractional or invalid | not checked | VA, rounded or 0 RD:3296-3322 | powerio warns where PowSybl carries it | small |
| kind tc, tct, patlt, other | not converted, silent PS/elements/OperationalLimitConversion.java:261-266 | retained as permanent or temporary with VA RD:3270-3282; balanced tc, tct to rate_c RD:4799-4803 | powerio does more | small |
| OperationalLimitType.direction low | rejected with invalid PS/elements/OperationalLimitConversion.java:419-431 | direction not read (FU); limit kept | powerio does more | small |
| several PATL in one set | lowest kept, fixed PS/elements/OperationalLimitConversion.java:338-354 | smallest kept, VA RD:3283-3292 | PowSybl warns too | small |
| several TATL with one duration | lowest kept, fixed PS/elements/OperationalLimitConversion.java:380-413 | all kept RD:3324-3331 | powerio does more | small |
| temporary limits without a permanent limit | permanent synthesized as a percentage of the lowest TATL PS/LimitsMapping.java:59-61, PS/Conversion.java:1035-1042, 1141 | permanent stays None RD:3238-3334; rate_a stays 0 RD:4800 | powerio defaults it | small |
| TATL name | shortName else name PS/elements/OperationalLimitConversion.java:424 | name RD:3269 | no gap | small |
| limit ids for SSH update | properties per limit PS/elements/OperationalLimitConversion.java:450-458 | RU that ids are regenerated RD:4915-4949 | powerio warns where PowSybl carries it | medium |
| CurrentLimit to balanced rate (MVA) | amperes kept per side | RD:4794-4798 sqrt(3) kV A / 1000 with the from bus kV for both terminals (RD:4478-4481 passes kv1 for a transformer); lowest across sets | powerio defaults it | small |
| ActivePowerLimit, ApparentPowerLimit | PS/elements/OperationalLimitConversion.java:67-71 | groups RD:3372-3376; balanced MW or MVA taken as MVA RD:4794-4798 | no gap | small |
| VoltageLimit lowVoltage, highVoltage on VoltageLevel | most restrictive consistent value, ids as properties, inconsistent pair ignored PS/elements/OperationalLimitConversion.java:57-61, 275-330 | RD:3064-3229 combined with VoltageLevel limits, inconsistent pair RU, combination VA | PowSybl warns too | small |
| **DC** | | | | |
| DCLineSegment resistance | NaN or negative to 0.1 with LOG.warn PS/elements/dc/DCLink.java:43-58; split or summed PS/elements/dc/DCConversion.java:285-296 | RD:2730 optional ohm | no gap | small |
| DCLineSegment inductance, capacitance, length | not read | RD:2731-2733 | powerio does more | small |
| DCLineSegment to balanced HvdcLine (R, NominalV, ConvertersMode, ActivePowerSetpoint, MaxP 1.2 times, LossFactor, PowerFactor) | islands, poles, links PS/elements/dc/DCConversion.java:54-70, 151-306; HvdcLineConversion.java:45-93; DCLinkUpdate.java:50-159 | no `Hvdc` row built (no hvdc reference in RD); DC kept only in DetailedConnectivity RD:2608-2922 | powerio drops it | large |
| unsupported DC configurations (back to back, multi terminal, inconsistent counts, line not in two ends, unvisited equipment) | reports and skip PS/elements/dc/DCIsland.java:26-83, DCConversion.java:108-112 | no configuration analysis; every record kept | powerio does more | large |
| DCConverterUnit operationMode, Substation | only for ratedUdc grouping PS/elements/dc/DCConversion.java:314-321 | RD:2613-2622; unknown or missing mode FormatRead RD:2409-2423 | powerio does more | small |
| DCNode, DCTopologicalNode (nominalV from unit ratedUdc, default 1.0) | PS/elements/dc/DCConversion.java:308-325 | RD:2624-2648 both, nominal_voltage_kv None | powerio defaults it | small |
| DCTerminal, ACDCConverterDCTerminal (sequenceNumber, nodes, polarity, SSH connected) | PS/elements/dc/DCMapping.java:26-36 (missing node reported); connected PS/elements/dc/AcDcConverterConversion.java:183-194 | RD:2193-2231 typed; unknown polarity FormatRead | powerio does more | small |
| ACDCConverter idleLoss, switchingLoss, resistiveLoss, PccTerminal | PS/elements/dc/AcDcConverterConversion.java:97-136 (PCC must map to a Branch or 3W, else report) | RD:2233-2254, 2834-2836 any terminal | no gap | small |
| ACDCConverter baseS, minP, maxP, minUdc, maxUdc, ratedUdc, valveU0, numberOfValves | only ratedUdc PS/elements/dc/DCLink.java:61-66 | RD:2826-2833 | powerio does more | small |
| SSH p, q, targetPpcc, targetUdc, pPccControl | reduced: PS/elements/dc/DCLinkUpdate.java:76-84, 134-145, HvdcConverterConversion.java:96-106; detailed: AcDcConverterConversion.java:196-228 | RD:2256-2289, 2838-2841 (six VSC and three CSC kinds) | powerio does more | small |
| VsConverter qPccControl, targetUpcc, targetQpcc | PS/elements/dc/HvdcConverterConversion.java:112-170, AcDcConverterConversion.java:230-251 | RD:2444-2461, 2849-2851 | no gap | small |
| VsConverter droop, droopCompensation, qShare, maxModulationIndex, maxValveCurrent, CapabilityCurve | curve only PS/elements/dc/HvdcConverterConversion.java:61 | RD:2844-2852, 2291-2299 | powerio does more | small |
| CsConverter operatingMode, ratedIdc, alpha and gamma limits and targets | operatingMode only PS/elements/dc/DCLinkUpdate.java:50-66 | RD:2425-2442, 2905-2912 | powerio does more | small |
| converter SV poleLossP, idc, uc, udc, delta, uf, uv, alpha, gamma | poleLossP for loss factors PS/elements/dc/DCLinkUpdate.java:86-94, 147-159 | RD:2853-2859, 2913-2918 | powerio does more | small |
| DCGround r, inductance; DCBusbar; DCSeriesDevice | DCGround r only (detailed) PS/elements/dc/DCGroundConversion.java | RD:2650-2696, 2736-2762; CGMES 2.4.15 ratedUdc derived RD:2463-2605 (VA or FormatRead) | powerio does more | small |
| DCSwitch, DCBreaker, DCDisconnector open | from SSH DCTerminal connected PS/elements/dc/DCSwitchConversion.java:36-62; same node invalid 31 | RD:2763-2802 Switch.open else derived | no gap | small |
| **Profiles, header, identity** | | | | |
| profiles accepted (EQ, TP, SSH, SV, EQ_BD, TP_BD, EQ_OP, SC) | all triples loaded; EquipmentCore required PS/Conversion.java:150-152; several profiles per file PS/CgmesImport.java:391-398 | any document not wholly DL, GL, DY merged RD:293-305; EQ and TP required RD:936-947 | no gap | small |
| DL, GL, DY documents | DL via a post processor CD/import.md:832-835 | skipped, per document RU summary RD:406-436, 894-897 | no gap | small |
| md:FullModel header (id, profiles, description, version, MAS, DependentOn, Supersedes) | CgmesMetadataModels PS/Conversion.java:563-603; a non integer version fixed to 1 643-650 | FU warnings only RD:307-404; first description and scenarioTime kept RD:863-893, 1361-1366 | powerio warns where PowSybl carries it | medium |
| scenarioTime, created (caseDate, forecastDistance) | PS/Conversion.java:545-555 | case_date RD:1366; created FU RD:331-338; forecast_distance not filled (net:1022) | powerio warns where PowSybl carries it | small |
| cgm-with-subnetworks | PS/CgmesImport.java:177-188, 289-341, 797-807 | single network, subnetworks empty RD:2100 | powerio drops it | large |
| CimCharacteristics | PS/Conversion.java:652-660 | source_model_format RD:1367; per VL topology kind RD:1712-1716 | no gap | small |
| base frequency | no record read; IIDM has no field | RD:1339-1349 BaseFrequency.frequency else 50 Hz with VD | powerio does more | small |
| base MVA | physical units, no base | RD:42, 1350-1357, 1365 100 MVA with VD | no gap | small |
| rdf:ID versus mRID (source-for-iidm-id) | PS/CgmesImport.java:448-455, 776-781 | XML:79-81 strips `urn:uuid:` and `_`; a mismatch with IdentifiedObject.mRID is FU RD:4867-4883 | no gap | small |
| decode-escaped-identifiers | PS/CgmesImport.java:456, 782-786 | none | powerio drops it | small |
| ensure-id-alias-unicity | PS/CgmesImport.java:730-734 | duplicate id FormatRead RD:485-510; conflicting values FormatRead RD:625-649 | no gap | small |
| IdentifiedObject.name, description, shortName (entsoe or eu) | name PS/elements/AbstractIdentifiedObjectConversion.java:29-34; shortName and description not read | name RD:2060-2062 (missing falls back to the id RD:619-622); shortName alias RD:2072-2080; description FU except GeneratingUnit RD:2026-2032 | powerio does more | small |
| aliases CGMES.Terminal1..3, RatioTapChanger1, PhaseTapChanger1, TransformerEnd1, RegulatingControl, OperationalLimitSet | PS/Conversion.java:1163-1178, 1206, 1211; PS/elements/AbstractConductingEquipmentConversion.java:714-723 | terminal mRID in `Terminal.component` RD:1879; end metadata RD:1983-1996; control property RD:1632-1638; TapChangerControl, tables, curves, limit types and limits regenerated RD:4915-4950 | powerio warns where PowSybl carries it | medium |
| CGMES.originalClass | PS/elements/SwitchConversion.java:97; PS/elements/AbstractConductingEquipmentConversion.java:250 | `cgmes_class` RD:1954-1962 | no gap | small |
| **Extensions on import** | | | | |
| ActivePowerControl | PS/elements/SynchronousMachineConversion.java:171 | RD:3784-3810, net:2838-2843 | no gap | small |
| RemoteReactivePowerControl, VoltageRegulation | PS/elements/AbstractReactiveLimitsOwnerConversion.java:204 | regulated_bus RD:3944-3960 (voltage only) | powerio warns where PowSybl carries it | large |
| LoadDetail | PS/elements/EnergyConsumerConversion.java:164 | class metadata | powerio drops it | large |
| ControlArea, TieFlow (Area, AreaBoundary, netInterchange, pTolerance, EIC) | PS/elements/ControlAreaConversion.java:30-49; PS/elements/TieFlowConversion.java:33-62; option PS/CgmesImport.java:741-745 | not consumed RD:4816-4839; `Bus.area` stays 1 (net:2054); the `Area` struct exists net:3234-3251 | powerio drops it | large |
| BaseVoltageMapping, CgmesMetadataModels, CgmesConversionContext | PS/Conversion.java:175-177, 563-603 | warnings only (see above) | powerio warns where PowSybl carries it | medium |
| SlackTerminal, ReferencePriorities, GeneratorEntsoeCategory, OperatingStatus, CgmesControlAreas, CgmesSvMetadata, CgmesSshMetadata, CgmesTapChangers, CgmesLineBoundaryNode | ReferencePriority only (PS/elements/SynchronousMachineConversion.java:129-132); CgmesTapChangers PS/elements/transformers/AbstractTransformerConversion.java:145-172; the others have no import usage in the conversion module (grep) | referencePriority kept and used for the slack RD:2017-2020, 3873-3877 | no gap | small |

### 4.2 CGMES export

| element or attribute | PowSybl (file:line) | powerio (file:line) | gap kind | fix size |
|---|---|---|---|---|
| **Profiles, header, options** | | | | |
| profiles written (profiles option) | EQ, TP, SSH, SV selectable PS/CgmesExport.java:249-274, 644-649; CGM export SSH per IGM plus SV 207-240 | always four WR:6995-7048; MOD:404-406 | powerio defaults it | small |
| cim-version | 16 or 100, default 16 or CimCharacteristics PS/CgmesExport.java:618-623, PS/export/CgmesExportContext.java:162-168 | MOD:405 always V3_0 (WR:3785 takes a version) | powerio defaults it | small |
| topology-kind | forced or detected (mixed: BUS_BRANCH for CIM16, NODE_BREAKER for CIM100) PS/export/CgmesExportContext.java:170-203 | mixed sets promoted to node breaker with `value_substituted` WR:915-925, 3859-3869 | powerio defaults it | medium |
| md:FullModel rdf:about | name based UUID PS/export/CgmesExportUtil.java:122-135 | WR:6994-6998 UUIDv5 from the network name | no gap | small |
| Model.scenarioTime, created | caseDate and now PS/export/CgmesExportUtil.java:150-155 | case date or STAMP with `value_defaulted` WR:6999-7006; created equals scenarioTime WR:1468-1472 | powerio defaults it | small |
| Model.description, version | extension or options PS/export/CgmesExportUtil.java:156-163; PS/CgmesExport.java:309-314, 730-734 | network name WR:1473-1477; version 1 WR:1478 | powerio defaults it | small |
| Model.DependentOn, Supersedes, boundary-eq-id, boundary-tp-id | PS/CgmesExport.java:302, 359-384; PS/export/CgmesExportUtil.java:168-171; PS/export/CgmesExportContext.java:216-234 | TP and SSH on EQ, SV on TP and SSH WR:7010-7047; no Supersedes, no boundary set ids | powerio drops it | small |
| Model.profile URIs (EQ_OP for CIM16 node breaker) | PS/export/CgmesExportUtil.java:172-180 | one URI per file WR:1422-1441, 1479 | powerio drops it | small |
| Model.modelingAuthoritySet | option, sourcing actor, or powsybl.org PS/export/CgmesExportContext.java:69, 205-214 | fixed `http://powerio.dev/cgmes` WR:1486-1489 | powerio defaults it | small |
| business-process, sourcing-actor, base-name, uuid-namespace, encode-ids, update-dependencies, cgm_export, naming-strategy | PS/CgmesExport.java:587-611, 613-769 | none; stem `powerio_<uuid>` WR:6994 | powerio defaults it | small |
| rdf:ID form and IdentifiedObject.mRID | `_` prefix and URL encoding, mRID element in CIM100 PS/export/CgmesExportUtil.java:187-210 | WR:1266-1272; no IdentifiedObject.mRID element | powerio drops it | small |
| IdentifiedObject.name (32 character truncation), shortName, isFictitious, description, EIC | truncation PS/export/CgmesExportUtil.java:212-217; EIC only on ControlArea | retained name, shortName, isFictitious WR:1273-1282, 1314-1323; no description or EIC | powerio does more | small |
| UUID generation for new objects | name based UUID under uuid-namespace PS/naming/CgmesNamingStrategy.java:36-39, 57-79 | UUIDv5 of `{kind}:{name}` under uuid5(NAMESPACE_URL, "https://powerio.dev/cgmes") WR:56-59; a source mRID kept when it parses as a UUID WR:62-65, 77-95, else `value_substituted` WR:202-207 | no gap | small |
| subordinate ids (terminals, ends, controls, tables, limit sets, SV records) | from aliases or properties else refs PS/naming/NamingStrategy.java:73-97; SV ids random PS/export/StateVariablesExport.java:343, 510, 676 | terminals WR:2078-2091; ends WR:97-153; SV deterministic WR:1327, 1357, 1397; controls, tables, limit types regenerated (RD:4915-4950) | powerio warns where PowSybl carries it | medium |
| **Hierarchy and topology** | | | | |
| GeographicalRegion, SubGeographicalRegion | from properties, Country, tags, reference data PS/export/CgmesExportContext.java:240-288 | fixed "GR" and "SGR" WR:4124-4130 | powerio defaults it | medium |
| Substation (Region, country) | PS/export/EquipmentExport.java:215-220 | WR:4187-4199; country, operator, tags `field_dropped` WR:4200-4208; fallback substation WR:4210-4220 | powerio drops it | medium |
| VoltageLevel (Substation, BaseVoltage, low and high limits) | PS/export/EquipmentExport.java:249-262 | WR:4221-4250, 4251-4261, 4507-4513 | no gap | small |
| BaseVoltage identity and several records per kV | one per distinct nominalV, id from BaseVoltageMapping or reference data PS/export/CgmesExportContext.java:290-333; boundary sourced not written PS/export/EquipmentExport.java:239-247 | one per kV, `det_mrid("basevoltage", kv)` WR:4131-4156 | powerio defaults it | medium |
| BaseFrequency | not written | WR:4164-4171 | powerio does more | small |
| ConnectivityNode | never in CIM16 bus branch; from adjacency groups or buses PS/export/EquipmentExport.java:124-168, 1783-1811 | WR:4285-4325; generated per bus WR:4384-4436; unconnected source nodes `record_dropped` WR:3941-3965 | no gap | small |
| Terminal EQ, SSH connected | PS/export/EquipmentExport.java:1738-1771; PS/export/SteadyStateHypothesisExport.java:136-156, 670-680 | WR:4571-4584, 4594-4596, 2366-2386 | no gap | small |
| Terminal TP for disconnected terminals | always (bus or connectable bus) PS/export/TopologyExport.java:132-145 | only when connected WR:4586-4593, 4614-4661 | powerio drops it | small |
| TopologicalNode | per bus PS/export/TopologyExport.java:417-422, 460-466; disconnected node breaker nodes get `<vl>_<node>` nodes 200-228 | WR:4326-4374, 4461-4475, 4515-4523; a container without a calculated bus is an `emission_error` WR:4063-4067 | powerio drops it | medium |
| TopologicalIsland, AngleRefTopologicalNode | one per synchronous component with an angle reference from ReferenceTerminals PS/export/StateVariablesExport.java:89-151, 300-319 | one island with every bus, the first `BusType::Ref` bus WR:6919-6934; `reference_missing` when none WR:6935-6940 | powerio defaults it | medium |
| load flow status (converged or diverged, mismatch thresholds) | PS/export/StateVariablesExport.java:102-106, 244-297; options PS/CgmesExport.java:693-717 | none | powerio drops it | medium |
| SvInjection for slack mismatch and fictitious loads | PS/export/StateVariablesExport.java:354-382, 392-396, 524-543 | none | powerio drops it | medium |
| SvVoltage | every TN, random id PS/export/StateVariablesExport.java:321-352; boundary nodes 328-340 | deterministic id, v and angle both required WR:1365-1403; no boundary nodes | powerio drops it | large |
| SvPowerFlow | every terminal, NaN as 0.0 PS/export/StateVariablesExport.java:384-428, 508-522 | only from retained terminal p and q WR:1334-1363, 4597-4610; partial `field_dropped` WR:1345-1355; branch solution WR:6394-6411 | powerio drops it (unsolved terminals get no record) | medium |
| SvStatus | PS/export/StateVariablesExport.java:650-685 | WR:1326-1332 | no gap | small |
| Switch EQ and SSH | PS/export/EquipmentExport.java:170-189; PS/export/CgmesExportContext.java:349-354; PS/export/SteadyStateHypothesisExport.java:652-664 | WR:4814-4850, 4944-4946, 6026-6035, 6105-6107 | powerio does more | small |
| SvPowerFlow for switches | PS/export/StateVariablesExport.java:425-427, 463-475 | WR:4866-4916, 6041-6096 | no gap | small |
| BusbarSection, Junction, Line container, Bay | PS/export/EquipmentExport.java:264-270 | WR:4664-4805, 4278-4283, 379-418 | powerio does more | small |
| boundary set export (EQ_BD and TP_BD references, dangling lines as boundary equipment, EquivalentInjection, reference data) | PS/export/EquipmentExport.java:1079-1292; PS/export/SteadyStateHypothesisExport.java:205-224, 682-698; PS/export/TopologyExport.java:316-415; PS/export/StateVariablesExport.java:401-414; PS/export/ReferenceDataProvider.java:57-188 | WR:555-578 record_dropped | powerio drops it | large |
| subnetworks | PS/CgmesExport.java:207-240 | WR:688-701 flattened with `value_collapsed` | powerio drops it | large |
| **Generators** | | | | |
| class dispatch (SynchronousMachine, ExternalNetworkInjection, EquivalentInjection, EnergySource) | PS/export/EquipmentExport.java:434, 446-470; PS/export/SteadyStateHypothesisExport.java:324-346 | WR:5341-5375 always SynchronousMachine | powerio drops it | medium |
| GeneratingUnit subclass, condensers | PS/export/elements/GeneratingUnitEq.java:56-71; PS/export/EquipmentExport.java:513 | WR:302-311, 5204-5264 | powerio does more | small |
| minOperatingP, maxOperatingP, initialP | PS/export/elements/GeneratingUnitEq.java:32-42 | WR:5245-5258 | no gap | small |
| SSH normalPF | PS/export/SteadyStateHypothesisExport.java:867-911 | WR:5271-5317 | no gap | small |
| ratedS | PS/export/EquipmentExport.java:509, 659-679 | WR:5355-5357 | powerio defaults it | small |
| SynchronousMachine.type, operatingMode | PS/export/CgmesExportUtil.java:472-527; PS/export/SteadyStateHypothesisExport.java:396-434 | WR:5346-5352, 5449-5455 | powerio defaults it | small |
| SSH p, q, referencePriority, controlEnabled | PS/export/SteadyStateHypothesisExport.java:339, 370-382 | WR:5417-5448 | no gap | small |
| RegulatingControl EQ and SSH | PS/export/CgmesExportUtil.java:537-556, 587-616; PS/export/EquipmentExport.java:437-444; PS/export/SteadyStateHypothesisExport.java:436-470, 623-642 | WR:5173-5191, 5391-5413, 5460-5497, 343-377 | no gap | small |
| shared RegulatingControl (several machines on one control) | PS/export/SteadyStateHypothesisExport.java:598-602 | WR:5177-5190; a possible duplicate rdf:ID caught by WR:7199-7203 (inferred, to be checked) | powerio drops it | small |
| ReactiveCapabilityCurve, minQ, maxQ | PS/export/EquipmentExport.java:535-598 | WR:2410-2476, 2570-2626, 5324-5354 | no gap | small |
| remote regulated bus without a terminal | terminal always known | WR:5499-5506 written as local regulation | powerio warns where PowSybl carries it | medium |
| windGenUnitType, HydroPowerPlant, FossilFuel | PS/export/elements/GeneratingUnitEq.java:46-49; PS/export/EquipmentExport.java:616-657 | not written | powerio drops it | small |
| cost, capability columns | none | WR:5507-5524 | no gap | small |
| **Loads** | | | | |
| load class (EnergyConsumer, ConformLoad, NonConformLoad, StationSupply) | PS/export/CgmesExportUtil.java:229-264 | WR:4980-4988 | powerio defaults it | small |
| EnergySource, AsynchronousMachine | PS/export/EquipmentExport.java:359-362, 417-426; PS/export/SteadyStateHypothesisExport.java:723-743 | not written | powerio drops it | medium |
| LoadGroup, LoadArea, SubLoadArea, EnergyArea | PS/export/EquipmentExport.java:275-293; PS/export/LoadGroups.java:29-55 | not written | powerio drops it | medium |
| LoadResponseCharacteristic | PS/export/EquipmentExport.java:375-415 | WR:5004-5097 | no gap | small |
| SSH EnergyConsumer.p, q | PS/export/SteadyStateHypothesisExport.java:745-755 | WR:5112-5122 | no gap | small |
| bus fictitiousP0, fictitiousQ0 | PS/export/EquipmentExport.java:295-351 | no field (net:2006-2039) | no gap | small |
| **Shunts and SVC** | | | | |
| shunt classes and attributes | PS/export/EquipmentExport.java:692-713 | WR:2121-2129, 5625-5710 | no gap | small |
| SSH sections; SvShuntCompensatorSections | PS/export/SteadyStateHypothesisExport.java:296-305; PS/export/StateVariablesExport.java:561-573 | WR:5725-5731, 5813-5817 (one count into both) | powerio drops it | medium |
| shunt RegulatingControl | PS/export/CgmesExportUtil.java:593-595; PS/export/SteadyStateHypothesisExport.java:310-319 | WR:5631-5648, 5732-5810 | no gap | small |
| EquivalentShunt | PS/export/EquipmentExport.java:684-689 | not written | powerio drops it | medium |
| StaticVarCompensator | PS/export/EquipmentExport.java:724-725; PS/export/elements/StaticVarCompensatorEq.java:48-59; PS/export/SteadyStateHypothesisExport.java:476-509 | WR:5910-6006, 343-377 (slope 0.0 at WR:5926) | powerio defaults it | small |
| **Lines and transformers** | | | | |
| ACLineSegment | PS/export/EquipmentExport.java:733-743 | WR:6233-6249 | no gap | small |
| SeriesCompensator | always ACLineSegment | WR:6162-6227, 6193 | powerio does more | small |
| PowerTransformerEnd placement | PS/export/EquipmentExport.java:873-917 | WR:6263-6266, 6315-6329 | no gap | small |
| PowerTransformerEnd ratedS | PS/export/elements/PowerTransformerEq.java:62-64 (always written) | never written | powerio drops it | medium |
| TransformerEnd.BaseVoltage | PS/export/elements/PowerTransformerEq.java:70 | never written | powerio drops it | small |
| export-transformers-with-highest-voltage-at-end1 | PS/export/EquipmentExport.java:802-871 | none | powerio drops it | small |
| `Branch.control` (2W automatic control) | not applicable | WR:6144-6150 `field_dropped` | powerio drops it | medium |
| fixed shift or ratio without a source tap changer | not applicable | WR:6332-6364, 6599-6662 synthesized one-step changer | powerio does more | small |
| source tap changers, tables, points | PS/export/EquipmentExport.java:942-1077 | WR:1755-1910 | no gap | small |
| tculControlMode | PS/export/elements/TapChangerEq.java:88-91 | not written (CGMES 3) | no gap | small |
| TapChangerControl EQ | PS/export/CgmesExportUtil.java:340-349; PS/export/EquipmentExport.java:969-998, 1053-1065 | WR:1802-1806, 1932 | powerio does more | small |
| TapChangerControl SSH | PS/export/SteadyStateHypothesisExport.java:527-582, 623-641 | WR:1938-1958 | no gap | small |
| SSH step; SvTapStep | PS/export/SteadyStateHypothesisExport.java:511-525; PS/export/StateVariablesExport.java:617-648 | WR:1869-1880 (SvTapStep only for solved changers) | powerio drops it | small |
| hidden combined tap changer (CgmesTapChangers) | PS/export/SteadyStateHypothesisExport.java:274-282, 584-589 | not applicable | no gap | small |
| three winding ends | PS/export/EquipmentExport.java:919-940 | WR:6512-6597; `Winding.control` dropped WR:6515-6526 | powerio drops it | medium |
| **Operational limits** | | | | |
| OperationalLimitSet per group | PS/export/EquipmentExport.java:1294-1367 | WR:6761-6850 | no gap | small |
| OperationalLimitType PATL, TATL | PS/export/EquipmentExport.java:1376-1398; PS/export/elements/OperationalLimitTypeEq.java:29-51 | WR:6853-6912 (CGMES 3 TATL without acceptableDuration, WR:6861-6867) | powerio defaults it | small |
| rate_a, rate_b, rate_c | not applicable | WR:6432-6471 (the kind `tc` written for rate_c is ignored by PowSybl, PS/elements/OperationalLimitConversion.java:261-266) | powerio does more | small |
| extra rating sets | not applicable | WR:6472-6483 `rating_set_dropped` | powerio drops it | medium |
| TATL name fallback | PS/export/EquipmentExport.java:1402-1404 | WR:2027, 1990-1994 | no gap | small |
| nonpositive limit values | written | WR:2000-2017 dropped | powerio does more | small |
| **DC** | | | | |
| HvdcLine (balanced) to DCLineSegment, converters, DC nodes | PS/export/EquipmentExport.java:1410-1472; PS/export/SteadyStateHypothesisExport.java:769-806; PS/export/StateVariablesExport.java:743-765 | WR:6965-6973 refused with `record_dropped` | powerio drops it | large |
| detailed DC classes | PS/export/EquipmentExport.java:1562-1677 | WR:2824-2880, 2901-3251, 3253-3545, 3548-3777, 3017-3049 | powerio does more | small |
| DCBusbar, DCSeriesDevice, DCLine, polarity | not written | WR:2891-2899, 2947-3003, 3098-3184, 2264-2280 | powerio does more | small |
| converter SSH | PS/export/SteadyStateHypothesisExport.java:960-1041 | WR:2773-2779, 3352-3357, 3639-3644 | powerio does more | small |
| converter SV | PS/export/StateVariablesExport.java:687-741 | WR:2628-2727 (omitted when any field is absent) | powerio drops it | small |
| DcNode nominal voltage, voltage, terminal p and i | PS/export/EquipmentExport.java:1670; PS/export/StateVariablesExport.java:697-698 | WR:588-601, 2937-2945, 3529-3538 `field_dropped` | powerio drops it | small |
| **Extensions on export** | | | | |
| SlackTerminal, ReferenceTerminals | PS/export/StateVariablesExport.java:114-151, 354-382 | WR:6922-6930 (the first `Ref` bus, one island) | powerio drops it | medium |
| ActivePowerControl | PS/export/SteadyStateHypothesisExport.java:891-895 | WR:5271-5278 | no gap | small |
| ReferencePriorities | PS/export/SteadyStateHypothesisExport.java:379-382 | WR:5434-5448 | no gap | small |
| CgmesTapChangers, RemoteReactivePowerControl, VoltageRegulation, LoadDetail | PS/export/CgmesExportUtil.java:232-280, 361, 537-591 | partial | powerio drops it | large |
| CgmesMetadataModels, CimCharacteristics, BaseVoltageMapping | PS/CgmesExport.java:295-305; PS/export/CgmesExportContext.java:165-174, 297-315 | none retained | powerio warns where PowSybl carries it | medium |
| ControlArea, TieFlow, EnergyArea; SSH netInterchange, pTolerance | PS/export/EquipmentExport.java:1679-1732; PS/export/SteadyStateHypothesisExport.java:936-958 | WR:6974-6987 areas dropped | powerio drops it | large |
| CgmesSvMetadata, CgmesSshMetadata, CgmesDanglingLineBoundaryNode, CgmesLineBoundaryNode, GeneratorEntsoeCategory, OperatingStatus | no export usage | not applicable | no gap | small |
| Equipment.inService in SSH | not written | WR:5120, 5268, 5457, 5729, 5974, 6421, 6732 | powerio does more | small |

Notes on mechanisms:

- Two winding conversion. PowSybl's `CgmesT2xModel` sums r and x and keeps g, b per end (PS/elements/transformers/CgmesT2xModel.java:44-52); defaults END1_END2, END1_END2, X (PS/Conversion.java:1133-1135); the structural ratio and end 2 tap changers move to end 1 with impedance corrections (ConvertedT2xModel.java:41-56, TapChangerConversion.java:277-383) and two changers of one kind are combined by fixing one (59-163). powerio sums per unit on each end's ratedU and folds ratedU and tap factors into `Branch.tap` (RD:4428-4467), keeping one `TapChanger` record per end (RD:3589-3656).
- Three winding. PowSybl legs use ratedU0 = ratedU1 by default (InterpretedT3xModel.java:44-62), rescaled on export (PS/export/EquipmentExport.java:924-931). powerio keeps `Transformer3W` with star impedances and writes them back per end (RD:4499-4554, WR:6534-6585).
- Limits. PowSybl attaches sets to branch sides, duplicates equipment level sets on lines, ignores them on transformers and injections, treats a missing duration as infinite, rejects low direction TATLs, and synthesizes a missing permanent limit (PS/elements/OperationalLimitConversion.java:46-56, 185-228, 332-336, 374-431; PS/LimitsMapping.java:59-61). powerio groups per (equipment, terminal) for any class, never reads direction, keeps every temporary limit, and converts current limits with the from bus kV (RD:3231-3399, 4794-4798).
- DC. PowSybl's reduced model detects point to point islands and derives mode, setpoint, and losses from SSH and SV (PS/elements/dc/DCConversion.java:92-268, DCIsland.java:26-93, DCLinkUpdate.java:50-159); powerio reads every detailed attribute (RD:2608-2922), derives CGMES 2.4.15 ratedUdc (RD:2463-2605), builds no balanced `Hvdc`, and refuses to write balanced `Hvdc` (WR:6965-6973).
- Identity. powerio: XML:79-81, RD:1146, 2081-2084, 4915-4950, WR:56-95. PowSybl: PS/naming/CgmesNamingStrategy.java:36-79.

### 4.3 powerio CGMES diagnostic emission sites

Mechanism: `CgmesDiagnostics::push` uses the collection's default code and `push_as` names a code (MOD:78-87); reader and writer records are copied into the shared `Diagnostics` with their own codes (MOD:150, 163, 418).

Reader warnings (RD). "RU (default)" is a bare `push` on the RD:840 or RD:3343 collection.

| file:line | code | message literal | condition |
|---|---|---|---|
| RD:316-320 | FU | "FullModel RDF identity `{}` in `{}` is not retained in the electrical model; fresh CGMES emission assigns a deterministic FullModel identity" | every document header |
| RD:324-328 | FU | "FullModel property `Model.modelingAuthoritySet` in `{}` is `{}`; ... is not retained" | header has modelingAuthoritySet |
| RD:332-336 | FU | "FullModel property `Model.created` in `{}` is `{}` and is not retained; ..." | header has created |
| RD:340-344 | FU | "FullModel property `Model.version` in `{}` is `{}` and is not retained; ..." | header has version |
| RD:360-364 | FU | "FullModel in `{}` has {} `Model.DependentOn` reference(s) [`{}`]{}; ..." | header has DependentOn |
| RD:378-382 | FU | "FullModel in `{}` has {} unmapped property value(s): {}; fresh CGMES emission omits them" | unmapped header properties |
| RD:396-400 | FU | "FullModel in `{}` has {} property value(s) encoded as nested RDF/XML: {}; ..." | nested header properties |
| RD:774-776 | VA | "{key} `{}` has no SSH p or q assignment; PowerIO used its complete SvPowerFlow terminal result p={} MW and q={} MVAr" | SSH absent, SvPowerFlow present |
| RD:780-782 | VA | "{key} `{}` has SSH p={} MW but no SSH q assignment; PowerIO kept p and used q={} MVAr ..." | SSH q absent |
| RD:786-788 | VA | "{key} `{}` has SSH q={} MVAr but no SSH p assignment; PowerIO kept q and used p={} MW ..." | SSH p absent |
| RD:792-794 | VD | "{key} `{}` has SSH p={} MW but no q assignment or complete SvPowerFlow terminal result; q was defaulted to 0 MVAr" | q absent everywhere |
| RD:798-800 | VD | "{key} `{}` has SSH q={} MVAr but no p assignment ...; p was defaulted to 0 MW" | p absent everywhere |
| RD:804-806 | VD | "{key} `{}` has neither SSH p/q assignments nor a complete SvPowerFlow terminal result; p and q were defaulted to zero" | no value at all |
| RD:818-822 | VA | "equipment `{}` has SvStatus.inService={} but SSH Equipment.inService={}; PowerIO used the solved SvStatus value" | SV and SSH disagree |
| RD:869-873 | VA | "FullModel `Model.scenarioTime` in `{}` is `{}`, which conflicts with the retained case date `{}`; ..." | documents disagree |
| RD:882-886 | VA | "FullModel `Model.description` in `{}` is `{}`, which conflicts with the retained network name `{}`; ..." | documents disagree |
| RD:933 | RU (default) | skipped part summary built at RD:406-436 | DL, GL, or DY document |
| RD:1206-1209 | RU (default) | "SvVoltage `{}` for boundary TopologicalNode `{}` belongs to modelingAuthoritySet `{}` and supplies {}; the node is shared by conducting equipment from modelingAuthoritySets [...]" | authority mismatch on a shared Line container node |
| RD:1211-1213 | RU (default) | "SvVoltage `{}` for TopologicalNode `{}` belongs to modelingAuthoritySet `{}`, while the node's ConnectivityNodeContainer belongs to `{}`; ..." | authority mismatch |
| RD:1228-1230 | RU (default) | "SvPowerFlow `{}` for terminal `{}` belongs to modelingAuthoritySet `{}`, while conducting equipment `{}` belongs to `{}`; ..." | authority mismatch |
| RD:1247-1249 | RU (default) | "SvPowerFlow `{}` for terminal `{}` has active power {} MW but no reactive power; CGMES requires both p and q, so the partial observation was not mapped" | SvPowerFlow with p only |
| RD:1250-1252 | RU (default) | "SvPowerFlow `{}` for terminal `{}` has reactive power {} MVAr but no active power; ..." | SvPowerFlow with q only |
| RD:1323-1327 | RU (default) | "no angle reference in the set (no TopologicalIsland, reference priority, or external injection); matrix consumers will report the missing slack" | no slack candidate |
| RD:1344-1347 | VD | "no BaseFrequency record; assuming 50 Hz" | no BaseFrequency |
| RD:1350-1356 | VD | "{}: per-unit values normalized onto a 100 MVA system base (CGMES carries none)" | always |
| RD:1476-1480 | FU | "{} `{}` property `ConductingEquipment.BaseVoltage` references BaseVoltage `{}` without a nominal voltage; ..." | referenced BaseVoltage has no nominalVoltage |
| RD:1496-1500 | FU | "{} `{}` states `ConductingEquipment.BaseVoltage` `{}` ({} kV), but none of its terminals resolve to a voltage level; ..." | equipment base voltage with no resolvable terminal |
| RD:1508-1512 | VA | "{} `{}` states `ConductingEquipment.BaseVoltage` {} kV, but its connected voltage level value(s) are {:?} kV; PowerIO uses the connected voltage levels ..." | stated base voltage differs from the connected level |
| RD:1544-1548 | VA | "{} distinct BaseVoltage identities [`{}`]{} all declare {} kV; PowerIO uses one source neutral voltage value ..." | several BaseVoltage ids share one kV |
| RD:1799-1801 | RU (default) | "BusbarSection {} has neither a ConnectivityNode nor a TopologicalNode and was skipped" | busbar terminal without topology |
| RD:1809-1811 | RU (default) | "BusbarSection {} has no unambiguous VoltageLevel through TopologicalNode {} and was skipped" | bus branch busbar without one voltage level |
| RD:1817-1819 | RU (default) | "BusbarSection {} TopologicalNode {} has no calculated bus and was skipped" | TN not among the buses |
| RD:1827-1829 | RU (default) | "BusbarSection {} has inconsistent topology and equipment containers and was skipped" | container mismatch |
| RD:2281-2283 | RU (default) | "{class} {}: {class}.pPccControl `{}` is unknown and was not assigned" | unknown pPccControl kind |
| RD:2321-2323 | RU (default) | "{class} {}: Curve.curveStyle `{}` is unknown and reactive limits were not assigned" | unknown curveStyle |
| RD:2328-2330 | RU (default) | "{class} {}: required Curve.curveStyle is absent and reactive limits were not assigned" | curveStyle absent |
| RD:2343-2345 | RU (default) | "{class} {}: {property} `UnitSymbol.{}` is unsupported; expected `UnitSymbol.{}`, so reactive limits were not assigned" | curve unit not W or VAr |
| RD:2350-2352 | RU (default) | "{class} {}: {property} is absent; expected `UnitSymbol.{}`, so reactive limits were not assigned" | curve unit absent |
| RD:2434-2436 | RU (default) | "CsConverter {}: CsConverter.operatingMode `{}` is unknown and was not assigned" | unknown operatingMode |
| RD:2453-2455 | RU (default) | "VsConverter {}: VsConverter.qPccControl `{}` is unknown and was not assigned" | qPccControl not voltagePcc or reactivePcc |
| RD:2537-2539 | VA | "CGMES 2.4.15 DCGround `{}` has no DCConductingEquipment.ratedUdc; derived {} kV from the unique positive ACDCConverter.ratedUdc in DCConverterUnit `{}`" | CIM16 ground without ratedUdc |
| RD:2600-2602 | VA | "CGMES 2.4.15 DCLineSegment `{}` has no DCConductingEquipment.ratedUdc; derived {} kV because terminal ... both units have the same unique positive ACDCConverter.ratedUdc" | CIM16 line without ratedUdc |
| RD:2652-2654 | RU (default) | "DCGround {}: missing DC terminal; skipped" | ground without DCTerminal |
| RD:2680-2682 | RU (default) | "DCBusbar {}: missing DC terminal; skipped" | busbar without DCTerminal |
| RD:2702-2704 | RU (default) | "DCLineSegment {}: fewer than two DC terminals; skipped" | line with fewer than two DC terminals |
| RD:2741-2743 | RU (default) | "DCSeriesDevice {}: fewer than two DC terminals; skipped" | series device with fewer than two |
| RD:2769-2771 | RU (default) | "{class} {}: fewer than two DC terminals; skipped" | DC switch with fewer than two |
| RD:2809-2811 | RU (default) | "VsConverter {}: fewer than two DC terminals; skipped" | VSC with fewer than two DC terminals |
| RD:2867-2869 | RU (default) | "CsConverter {}: fewer than two DC terminals; skipped" | CSC with fewer than two DC terminals |
| RD:2946-2948 | RU (default) | "StaticVarCompensator {}: no terminal on a topological node; skipped" | SVC terminal not on a bus |
| RD:2973-2975 | RU (default) | "StaticVarCompensator {}: zero or missing inductive/capacitive rating has no finite susceptance; using 0 S for that bound" | rating zero or absent |
| RD:3072-3075 | RU | "VoltageLimit `{}` has no OperationalLimitSet and was not mapped" | VoltageLimit without a set |
| RD:3079-3083 | RU | "VoltageLimit `{}` in OperationalLimitSet `{}` does not target equipment or a terminal in one VoltageLevel and was not mapped" | set target not resolvable to a VL |
| RD:3088-3091 | RU | "VoltageLimit `{}` has no OperationalLimitType and was not mapped" | no type |
| RD:3099-3103 | RU | "VoltageLimit `{}` has no finite positive normalValue or value and was not mapped" | value invalid |
| RD:3116-3121 | RU | "VoltageLimit `{}` has OperationalLimitType kind `{}` instead of lowVoltage or highVoltage and was not mapped" | kind not lowVoltage or highVoltage |
| RD:3140-3147 | VA | "VoltageLevel `{}` declares lowVoltageLimit {} kV above highVoltageLimit {} kV; both inconsistent VoltageLevel limits were ignored" | VL limits reversed |
| RD:3164-3171 | RU | "VoltageLimit records [{}] for VoltageLevel `{}` form an inconsistent pair: low {} kV is above high {} kV; the pair was not mapped" | VoltageLimit pair reversed |
| RD:3190-3195 | RU | "VoltageLimit records [{}] conflict with the valid VoltageLevel `{}` range; the VoltageLimit records were not mapped" | combination reversed |
| RD:3204-3209 | VA | "VoltageLimit records [{}] were combined with VoltageLevel `{}` into its most restrictive valid lowVoltageLimit/highVoltageLimit pair; ..." | VoltageLimit applied |
| RD:3248-3251 | RU (default, RD:3343) | "{class} `{}` has no OperationalLimitType and was not retained" | loading limit without type |
| RD:3258-3261 | RU (default, RD:3343) | "{class} `{}` has a missing, nonfinite, or nonpositive value and was not retained" | invalid value |
| RD:3273-3279 | VA | "{class} `{}` uses OperationalLimitType kind `{}`; PowerIO retains it as a {} limit and fresh CGMES emits kind `{}`" | kind not patl or tatl as classified |
| RD:3285-3287 | VA | "OperationalLimitSet `{}` contains several permanent {class} values; the smallest is retained" | several PATL |
| RD:3301-3306 | VA | "{class} `{}` uses OperationalLimitType.acceptableDuration={} seconds, which cannot be represented as a nonnegative whole-second duration; PowerIO retains the limit with 0 seconds ..." | duration nonfinite, negative, or huge |
| RD:3313-3318 | VA | "{class} `{}` uses OperationalLimitType.acceptableDuration={} seconds; PowerIO rounds it to {} whole seconds ..." | fractional duration |
| RD:3367-3369 | RU (default, RD:3343) | "OperationalLimitSet `{}` has neither a Terminal nor Equipment target" | set without a target |
| RD:3413-3417 | VA | "tap changer `{}` RegulatingControl.mode `{}` has no PowerIO tap regulation mode; the typed control has no mode and fresh CGMES output selects the default for the tap changer kind" | unknown tap control mode |
| RD:3668-3670 | RU (default) | "LoadResponseCharacteristic `{}` has a nonfinite {} coefficient and was not applied" | nonfinite ZIP coefficient |
| RD:3675-3677 | RU (default) | "LoadResponseCharacteristic `{}` has {} coefficients that sum to zero and was not applied" | zero sum |
| RD:3681-3683 | VA | "LoadResponseCharacteristic `{}` has {} coefficients that sum to {}; they were normalized to one" | sum not one |
| RD:3767-3769 | RU (default) | "{class} {}: no terminal on a topological node; skipped" | load terminal not on a bus |
| RD:3833-3835 | RU (default) | "SynchronousMachine {}: no terminal on a topological node; skipped" | machine terminal not on a bus |
| RD:3890-3894 | VA | "ExternalNetworkInjection `{}` is represented as a balanced generator; fresh CGMES output emits a SynchronousMachine ..." | every ExternalNetworkInjection |
| RD:3931-3933 | RU (default) | "SynchronousMachine {} uses RegulatingControl.mode `{}`; the exact control is retained for CGMES emission but Generator voltage regulation fields do not model it" | control mode not voltage |
| RD:3990-3992 | VD | "RegulatingControl {} has unsupported targetValueUnitMultiplier `{}`; targetValue is interpreted as kV" | multiplier not none, m, k, M, G |
| RD:4094-4096 | RU (default) | "NonlinearShuntCompensator {}: section point numbers do not cover 1 through {}" | point numbering gaps |
| RD:4181-4185 | FU | "`SvShuntCompensatorSections.sections` for `{}` is {} while the SSH `ShuntCompensator.sections` assignment is {}; the shunt keeps the SSH assignment ..." | SV and SSH sections disagree |
| RD:4234-4239 | VA | "{} switch(es) internal to one topological node are represented by the topology itself" | switches joining one TN |
| RD:4278-4283 | VA | "{} EquivalentInjection(s) at boundary nodes mapped to loads/generators (p/q at the tie point)" | any EquivalentInjection read |
| RD:4298-4301 | RU (default) | "ACLineSegment {}: terminals do not land on two topological nodes (boundary line without its boundary set?); skipped" | line terminal without a bus |
| RD:4331-4333 | RU (default) | "SeriesCompensator {}: terminals do not land on two topological nodes; skipped" | series compensator terminal without a bus |
| RD:4396-4398 | RU (default) | "{} PowerTransformer record(s) have neither two nor three windings and were skipped" | end count not 2 or 3 |
| RD:4419-4421 | RU (default) | "PowerTransformer {}: ends do not land on topological nodes; skipped" | 2W end without a bus |
| RD:4580-4582 | VD | "PowerTransformer `{}` end `{}` PowerTransformerEnd.ratedU {}; used the connected topological node base voltage {} kV for impedance conversion" | ratedU absent or nonpositive |
| RD:4599-4601 | RU (default) | "PowerTransformer {}: winding {} has no terminal; skipped" | 3W end without a terminal |
| RD:4612-4614 | RU (default) | "PowerTransformer {}: winding {} does not land on a topological node; skipped" | 3W end without a bus |
| RD:4724-4726 | RU (default) | "{other} {}: phase tap changer type is not supported in the calculation projection; using zero shift" | unknown phase tap class |
| RD:4872-4880 | FU | "{} `{}` property `IdentifiedObject.mRID` is {}, but its RDF identity is `{}`; PowerIO uses the RDF identity and fresh CGMES output replaces this mRID" | mRID differs from the rdf id |
| RD:4894-4896 | RU (default) | "{count} {class} object(s) have no electrical or hierarchy mapping; only their identity metadata is retained" | per class not consumed |
| RD:4905-4909 | FU | "{} {class} object(s) provide {} `{property}` value(s) that PowerIO does not map (objects: [`{}`]{}); fresh CGMES output omits this field" | per (class, property) not read on a consumed class |
| RD:4945-4947 | RU (default) | "{} {class} {identity} [`{}`]{} {verb} not retained as PowerIO component IDs; ... fresh CGMES assigns deterministic subordinate mRIDs" | any of the twelve subordinate classes present |

Reader hard errors (FormatRead, code `PARSE.SOURCE.MALFORMED`), condensed:

| file:line | message literal | condition |
|---|---|---|
| RD:237-241, 282-286 | "{class} `{id}` property {property} is an RDF resource reference, not a typed literal"; "... has invalid value `{text}`; expected {expected}" | literal given as a resource, or unparsable |
| RD:479-481, 496-500, 506-508 | "{class} object has no rdf:ID or rdf:about identifier"; "RDF identifier `{id}` is used for both {} and {class}"; "RDF identifier `{id}` is defined more than once" | identity problems |
| RD:635-639 | "RDF object `{object}` assigns conflicting `{property}` values: {} and {}" | conflicting values across profiles |
| RD:911-913, 918-922, 943-945 | "no file declares a CIM16/CIM100 namespace; not a CGMES set"; "one profile set declares more than one CGMES release: {}"; "CGMES profile set is missing required {missing} profile data" | namespace and profile checks |
| RD:1002-1006, 1011-1015 | "{} `{}` property {property} is not an RDF resource reference"; "... references missing `{target}`" | required reference invalid or dangling |
| RD:1122-1141, 1152-1156 | TopologicalNode without BaseVoltage, BaseVoltage without value, nonpositive kV, or no TopologicalNode records | topology base voltages |
| RD:1171-1175, 1238-1242, 1259-1283 | conflicting SvVoltage, conflicting SvPowerFlow, SvStatus without equipment or boolean, conflicting SvStatus | SV conflicts |
| RD:1921-1923 | "switch `{id}` terminal has no topology connection" | detailed switch terminal without CN or TN |
| RD:2218-2222, 2243-2245, 2385-2387, 2414-2420 | unknown DC terminal polarity, PCC sequence number above 255, invalid unsigned integer, unknown or missing DCConverterUnit.operationMode | DC record validation |
| RD:2479-2595 | CGMES 2.4.15 ratedUdc derivation failures for DCGround and DCLineSegment (invalid, missing, conflicting converter values; ground outside a unit; line without two terminals; terminal without a node; node outside a unit; endpoints disagree) | CIM16 DC derivation |
| RD:3528-3530 | "tap changer `{tap}` has an invalid low/high step range" | high below low or more than 10000 steps |
| RD:3791-3803 | "GeneratingUnit {} has a nonnumeric normalPF `{text}`"; "... has invalid normalPF `{text}`; expected a finite nonnegative distributed slack participation factor" | normalPF invalid |
| RD:3852-3855 | message from `calc_reactive_limits_at_active_power` (net:570-584) | invalid capability curve |
| RD:1386 | `ComponentId::new` error | invalid component id text |

Writer warnings (WR):

| file:line | code | message literal | condition |
|---|---|---|---|
| WR:177-182 | field_dropped | "component `{}` has more than one CGMES short name; emitted `{}` and omitted `{}`" | several short_name aliases |
| WR:189-194 | field_dropped | "component `{}` alias `{}` of type `{}` has no CGMES IdentifiedObject.shortName mapping" | alias of another type |
| WR:203-206 | value_substituted | "component `{}` has non-UUID CGMES identifier `{}`; fresh CGMES uses a deterministic UUID" | CGMES external id not a UUID |
| WR:209-214 | field_dropped | "component `{}` external identifier `{}` from authority `{}` has no CGMES IdentifiedObject field" | non CGMES external identifier |
| WR:361-363 | value_substituted | "equipment `{}` has unsupported RegulatingControl.targetValueUnitMultiplier `{}`; fresh CGMES output uses UnitMultiplier.k" | retained multiplier unknown |
| WR:394-400 | field_dropped | "component `{}` references source EquipmentContainer ... the EquipmentContainer reference was omitted" | container unknown and no fallback |
| WR:405-411 | value_substituted | "component `{}` references source EquipmentContainer ... fresh CGMES uses the equipment terminal's VoltageLevel instead" | container not a VoltageLevel (Bay, Line) |
| WR:564-567 | record_dropped | "BoundaryLine `{}` is retained in PowerIO detailed connectivity but fresh CGMES does not emit EQBD or TPBD boundary records; ..." | any detailed dangling line |
| WR:574-577 | record_dropped | "TieLine `{}` joining BoundaryLines `{}` and `{}` is retained ... fresh CGMES emits neither the TieLine nor its boundary records; ..." | any tie line |
| WR:582-585 | field_dropped | "ConnectivityNode `{}` has source node number {}; CGMES identifies connectivity nodes by mRID and has no node number field" | node_number present |
| WR:590-593 | field_dropped | "DCNode `{}` has nominal_voltage_kv={}; CGMES DCNode has no nominal voltage field" | DcNode nominal voltage |
| WR:596-599 | field_dropped | "DCNode `{}` has voltage_kv={}; the emitted EQ, TP, SSH, and SV profiles have no DCNode voltage field" | DcNode voltage |
| WR:611-614 | field_dropped | "{class} `{}` DC terminal `{}` has polarity `{}`; CGMES defines ACDCConverterDCTerminal.polarity only for converter terminals" | polarity on a non converter DC terminal |
| WR:646-650 | value_substituted | "VsConverter `{}` has conflicting valve voltages uf={} kV and uv={} kV; {} writes {}={} kV and does not emit {}={} kV" | uf and uv differ |
| WR:661-666 | field_dropped | "component `{}` metadata property `{}` has no fresh CGMES mapping" | metadata property not in `metadata_property_is_used` |
| WR:672-675 | field_dropped | "operational limit group `{}` on equipment `{}` metadata property `{}` has no fresh CGMES mapping" | limit group property |
| WR:680-684 | field_dropped | "omitted field record for component `{}` field `{}` does not match a CGMES assignment the writer can suppress ..." | omission record without a matching component |
| WR:695-700 | value_collapsed | "subnetwork `{}` with parent `{}` and {} component reference(s) is flattened into the single fresh CGMES model set; ..." | any subnetwork |
| WR:703-705, 3825-3827 | field_dropped | "network case metadata `{}` has no field in the fresh CGMES EQ, TP, SSH, or SV FullModel headers" | case metadata field present |
| WR:1055-1057 | record_dropped (default) | "mixed topology projection preserves transformer `{}` terminal {} as connected at configured bus `{}`, but its projected ConnectivityNode is shared only with converter(s) [...] in a DC island ..." | transformer terminal shares a projected CN only with converters |
| WR:1345-1347, 1351-1353 | field_dropped | "terminal `{}`: retained active power field {} MW without reactive power; CGMES SvPowerFlow requires both p and q, ..."; reactive without active | p without q, q without p |
| WR:1375-1377 | field_dropped | "TopologicalNode `{}`: retained SvVoltage belongs to a different modeling authority than its source equipment; ... was not emitted" | authority mismatch property set |
| WR:1385-1387, 1391-1393 | field_dropped | "TopologicalNode `{}`: retained voltage field {} kV without an angle; ..."; angle without voltage | v without angle, angle without v |
| WR:2001-2004, 2009-2016 | record_dropped | "operational limit set `{}` {} permanent limit `{}` was not emitted because a CGMES limit must be positive and finite"; temporary limit variant | nonpositive or nonfinite limit |
| WR:2348-2350 | reference_missing | "converter `{}` has no AC Terminal record; its DC equipment was emitted without an AC connection" | converter without a Terminal record |
| WR:2354-2357 | record_dropped (default) | "converter `{}` terminal {} has no calculated AC bus and was not emitted" | converter terminal without a bus |
| WR:2427-2431 | record_dropped | "{class} `{}` has an empty reactive capability curve and was not emitted" | curve with no points |
| WR:2441-2445, 2503-2507 | field_dropped | "{class} `{}`: reactive capability curve properties have no CGMES field"; "equipment `{}`: min/max reactive limits properties have no CGMES field" | properties present |
| WR:2600-2602 | value_substituted | "generator `{}`: typed reactive limits evaluate to minQ={} MVAr and maxQ={} MVAr at p={} MW, while the balanced generator row contains minQ={} ..." | typed limits differ from the row |
| WR:2661-2665, 2709-2713 | record_dropped (default) | "VsConverter `{}`: converter SV object omitted because {} is absent"; CsConverter variant | any SV field absent |
| WR:2938-2943, 2995-3000, 3089-3094, 3176-3181 | field_dropped | "DCGround `{}`: CGMES DCTerminal has no active power or current field"; DCBusbar, DCLineSegment, DCSeriesDevice variants | DC terminal p or i |
| WR:2970-2975 | field_dropped | "DCBusbar `{}`: rated DC voltage has no CGMES 2.4.15 field" | CIM16 busbar with ratedUdc |
| WR:3242-3248 | field_dropped | "DC switch `{}`: resistance {} ohm has no CGMES DC switch field" | nonzero switch resistance |
| WR:3523-3526, 3760-3763 | field_dropped | "VsConverter `{}`: PowerIO's segmented droop curve is not the CGMES scalar droop and was not emitted"; CsConverter variant | droop_curve present |
| WR:3534-3537, 3771-3774 | field_dropped | "VsConverter `{}`: XIIDM DC terminal current and active power have no CGMES DCTerminal fields; ..."; CsConverter variant | DC terminal p or i |
| WR:3754-3757 | field_dropped | "CsConverter `{}`: reactive model and power factor have no direct CGMES field; PCC p/q carries the available operating assignment" | reactive_model or power_factor present |
| WR:3836-3841 | value_collapsed | "system base {} MVA: CGMES carries no MVA base, so a reparse lands per-unit values on 100 MVA" | base MVA not 100 |
| WR:3865-3867 | value_substituted | "source detailed connectivity contains both node breaker and bus breaker VoltageLevels; fresh CGMES emission promotes {} bus breaker VoltageLevel(s) to node breaker connectivity ..." | mixed topology kinds |
| WR:3941-3943, 3961-3963, 3986-3988 | record_dropped | "source CGMES ConnectivityNode `{}` belongs to typed bus breaker VoltageLevel `{}` and was omitted ..."; "... is not connected to a calculated bus or retained equipment and was omitted ..."; TopologicalNode variant | unconnected or bus breaker source nodes |
| WR:4204-4206 | field_dropped | "substation `{}`: country, operator, and geographical tags are not emitted by the CGMES core equipment profile" | substation with any of those |
| WR:4215-4218 | value_defaulted | "voltage levels without a declared substation were placed in the PowerIO substation" | VoltageLevel without a substation |
| WR:4273-4275 | value_defaulted | "balanced bus {} is absent from detailed connectivity; fresh CGMES emitted its TopologicalNode and a generated VoltageLevel and ConnectivityNode" | bus not in detailed connectivity |
| WR:4319-4321, 4477-4479 | value_defaulted | "source ConnectivityNodeContainer `{}` for {} was not a typed VoltageLevel; ... placed in generated VoltageLevel `{}`"; TopologicalNode variant | container not a VoltageLevel |
| WR:4432-4434 | value_defaulted | "at least one balanced equipment terminal at bus {} is absent from detailed connectivity; fresh CGMES emitted a generated ConnectivityNode for that terminal" | terminal without a detailed record |
| WR:4544-4548 | field_dropped | "bus {}: emergency voltage band (evhi/evlo) has no CGMES slot" | emergency band present |
| WR:4672-4674 | record_dropped | "BusbarSection `{}` belongs to bus breaker VoltageLevel `{}`; it remains in PowerIO detailed connectivity but was omitted from fresh CGMES ..." | busbar in a bus breaker level |
| WR:4853-4857, 6098-6102 | field_dropped | "switch `{}` thermal rating {} MVA was not emitted: CGMES Switch carries rated current, not an apparent power limit" | switch thermal rating (detailed and balanced) |
| WR:4952-4954 | value_collapsed | "{} internal connection(s) have no distinct CGMES equipment record; their calculated topology is retained through TopologicalNode assignments" | internal connections present |
| WR:5041-5043 | field_dropped | "load at bus {}: nominal voltage, source load type, and scaling metadata have no LoadResponseCharacteristic fields and were omitted" | ZIP model with those fields |
| WR:5075-5077 | field_dropped | "load at bus {}: nominal voltage has no LoadResponseCharacteristic field and was omitted" | exponential model with a nominal voltage |
| WR:5153-5158, 5573-5578, 5857-5862 | value_substituted | "{generator, shunt, static var compensator} `{}` source RegulatingControl.enabled={} and RegulatingCondEq.controlEnabled={} represented ... regulation {}; the typed value is {}, so fresh CGMES output sets both enable flags to ..." | typed flag differs from the source flags |
| WR:5247-5249 | field_dropped | "GeneratingUnit `{}` source initialP `{}` has no CGMES 3.0 property" | retained initialP in CIM100 output |
| WR:5288-5292, 5295-5297, 5300-5304, 5314-5316 | field_dropped | "generator `{}`: participate=false{} cannot be represented by CGMES GeneratingUnit.normalPF"; "... participate=true without a participation factor ..."; "... active power control droop has no CGMES property"; "... active power control target limits have no CGMES property" | ActivePowerControl fields |
| WR:5502-5505 | value_substituted | "generator `{}` names remote regulated bus {} without an exact regulating terminal; CGMES output uses the generator terminal" | regulated_bus without a terminal |
| WR:5508-5512, 5517-5521 | field_dropped | "generator at bus {}: cost curves have no CGMES slot"; "generator at bus {}: capability/ramp columns have no CGMES slot" | cost or caps present |
| WR:5536-5539 | field_dropped | "storage `{}`: active power control has no CGMES battery mapping" | storage with active power control |
| WR:5610-5612 | value_substituted | "shunt `{}` assigned section count {} does not match the section count {} calculated from its conductance and susceptance; fresh CGMES uses the explicit assignment" | section count mismatch |
| WR:5620-5622 | value_substituted | "shunt at bus {}: its assigned conductance and susceptance do not equal a prefix of its control blocks; CGMES uses the closest section count {}" | g, b not a block prefix |
| WR:5806-5808 | value_substituted | "shunt at bus {}: remote regulated bus {} is written as local regulation because the balanced model does not identify a terminal on the regulated equipment" | remote bus without a terminal |
| WR:6133-6138 | value_collapsed | "branch {} ({}-{}): asymmetric terminal charging folded into the symmetric bch/gch totals" | asymmetric charging on a line |
| WR:6144-6150 | field_dropped | "branch {} ({}-{}): automatic tap/phase control data is not written (fixed in-service step only)" | `Branch.control` present |
| WR:6193-6195 | value_substituted | "SeriesCompensator `{}` has nonzero shunt charging and is written as ACLineSegment: ...; {}" | retained series compensator with charging |
| WR:6277-6284 | value_substituted | "branch {} ({}-{}): fixed phase shift {} degrees differs from the retained tap changer position value {} degrees; the tap changer definition was emitted" | shift differs from the source step |
| WR:6473-6482 | rating_set_dropped | "branch {} ({}-{}): extra rating sets / current ratings beyond A/B/C have no CGMES slot" | rating_sets or current_ratings present |
| WR:6516-6525 | field_dropped | "three winding transformer `{}` winding {}: automatic transformer control {:?} (...) has no CGMES writer mapping and was not emitted" | `Winding.control` present |
| WR:6552-6555, 6561-6564 | value_substituted | "three winding transformer `{}` winding {}: fixed tap ratio {} differs from the retained tap changer position value {}; ..."; phase shift variant | tap or shift differs from the source step |
| WR:6752-6758 | record_dropped | "tap changer on `{}` winding {} was not emitted: {message}" | `write_source_tap_changer` returned an error text |
| WR:6763-6766, 6770-6773 | record_dropped (default) | "operational limit group `{}` targets unknown equipment `{}` and was not emitted"; "... has terminal 0 and was not emitted" | group on unknown equipment or terminal 0 |
| WR:6786-6789 | field_dropped | "operational limit group `{}`: active and apparent power limits belong to the CIM16 EquipmentOperation profile and were omitted from the four-profile CGMES 2.4.15 output" | CIM16 output with those limits |
| WR:6845-6848 | field_dropped | "operational limit group `{}`: CGMES has no selected-group property; all groups were emitted" | selected true |
| WR:6936-6939 | reference_missing | "no reference bus: the SV island has no angle reference" | no `BusType::Ref` bus |
| WR:6957-6960 | record_dropped | "equipment reactive limits for `{}` were not emitted: the CGMES writer has no reactive limits association for component type `{}`" | limits on an unsupported component type |
| WR:6970-6972 | record_dropped (default) | "HVDC line `{}` is a two terminal calculation record, not a physical CGMES DC network, and was not emitted. ..." | any balanced `Hvdc` row |
| WR:6983-6985 | record_dropped (default) | "{count} {what}(s) have no CGMES mapping yet and are dropped" | storage units, area records, or a solver block present |
| WR:7000-7004 | value_defaulted | "case date is absent; CGMES Model.scenarioTime and Model.created use 2000-01-01T00:00:00Z" | case_date None |

Writer hard errors (`emission_error`, FormatRead "CGMES", WR:1503-1508), condensed: duplicate end identity (WR:148-150); active power control fields nonfinite, negative, outside limits, or reversed (WR:1529-1565); unknown bus or bad base kV (WR:2045-2049, 3796-3798, 3807-3809, 4064-4093); DC records without containment, DCTopologicalNode, container, required property, sequence number, DCNode, connected flag, or CIM100 polarity (WR:2141-2278); duplicate or conflicting reactive limits (WR:2490-2492, 2523-2525); converter control modes absent or unrepresentable (WR:3353-3407, 3442-3445, 3640-3704); network validation failure (WR:3787-3789); connected busbar, junction, or switch endpoint without a bus (WR:4733-4735, 4794-4796, 4936-4938); regulating terminal not identifiable (WR:5396-5398, 5747-5749); shunt sections above maximum (WR:5604-5606); output XML and RDF graph validation (WR:7070-7202).

Reader source errors (MOD:225-375; FormatRead "CGMES"): no CGMES XML documents; archive size, entry count, symlink, nested archive, decompression limit, unsafe entry name; more than the document limit; 64 MiB cumulative limit; duplicate normalized name; invalid UTF-8; DTD or entity declaration. XML:121-126 wraps the XML parser's own error text.

Shared passes for the CGMES target: an unchanged CGMES source is re-committed verbatim with no diagnostics (fmt:1168-1209); otherwise `cgmes::artifacts` (MOD:404-421) runs `write_cgmes` at V3_0 after `apply_emit_cost_policy` (fmt:1232-1246, 1331-1367). `emit_text_with_options` returns `WriteUnsupported` for CGMES (fmt:1123-1125), so the shared passes at fmt:1127-1131 do not run; the writer covers the reference bus (WR:6936-6939) and charging (WR:6133-6138) itself, and bus locations get no writer diagnostic.

### 4.4 CGMES fixes in priority order

1. Balanced HVDC: the reader builds no `Hvdc` row from CGMES DC equipment and the writer refuses balanced `Hvdc` rows (WR:6965-6973), so a CGMES case with a DC link loses its DC power transfer in every balanced target; PowSybl's reduced model (PS/elements/dc/DCConversion.java:54-306) is the reference for island, pole, and link detection. Large.
2. Boundary set: no boundary node classification, no dangling line or tie line on read, and dangling lines dropped on write (RD:2114-2115, WR:555-578). Large.
3. Areas: ControlArea and TieFlow are not consumed and areas are dropped on write (RD:4816-4839, WR:6974-6987) although `Area` exists in the model (net:3234-3251). Large.
4. Current limit conversion on transformers uses the end 1 kV for both terminals (RD:4478-4481, 4794-4798); a missing acceptableDuration becomes 0 seconds (RD:3294-3295) where PowSybl uses infinite; fresh TATL types in CGMES 3 output carry no acceptableDuration (WR:6861-6867); the kind `tc` written for rate_c is ignored by PowSybl. Small each.
5. Missing minQ/maxQ become 0 (RD:3844-3845), missing minP/maxP become 0 (RD:3868-3869), and RegulatingControl modes other than voltage are not typed (RD:3929-3936); EquivalentInjection and ExternalNetworkInjection limits are dropped (RD:3898-3901, 4253-4275). Small to large.
6. Classes PowSybl converts that powerio only counts: EquivalentBranch, AsynchronousMachine, EnergySource, EquivalentShunt, Ground, SvInjection, ControlArea, TieFlow (RD:1045-1101, 4893-4896). Small to medium each.
7. Export omissions PowSybl's importer would notice: `PowerTransformerEnd.ratedS` (mandatory in PowSybl's writer, PS/export/elements/PowerTransformerEq.java:62-64), `TransformerEnd.BaseVoltage`, `IdentifiedObject.mRID`, SvTapStep for unsolved changers, TP records for disconnected terminals, and a possible duplicate rdf:ID when two machines share one source RegulatingControl (WR:5177-5190, inferred, to be checked by the worker).

---

## 5. The conversion matrix warnings

Source: `/home/sam/Research/powerio/powerio-cli/tests/conversion_matrix_report.rs`. No recorded warning text exists in the repository; the details markdown is produced at run time and uploaded as a CI artifact by `.github/workflows/conversion-matrix.yml`. The warning texts below are the message literals at the emission sites, which is what the run time report prints.

### 5.1 The baseline arrays

`TRANSMISSION_WARNING_BASELINE` (conversion_matrix_report.rs:713-722), rows are sources and columns are targets, both in the order MATPOWER .m, PowerModels JSON, PSS/E .raw (revision 33), PowerWorld .aux, egret JSON, pandapower JSON, Surge JSON, PSLF .epc. Each cell is the sum over the six cases of source parse, target write, and target readback warnings.

| source / target | MATPOWER | PowerModels | PSS/E | PowerWorld | egret | pandapower | Surge | PSLF |
|---|---|---|---|---|---|---|---|---|
| MATPOWER .m | 0 | 1 | 15 | 15 | 7 | 15 | 23 | 14 |
| PowerModels JSON | 0 | 0 | 15 | 14 | 6 | 14 | 22 | 13 |
| PSS/E .raw | 1 | 1 | 0 | 2 | 1 | 3 | 1 | 2 |
| PowerWorld .aux | 0 | 0 | 0 | 0 | 0 | 2 | 0 | 0 |
| egret JSON | 0 | 0 | 9 | 9 | 0 | 8 | 1 | 7 |
| pandapower JSON | 1 | 1 | 8 | 8 | 1 | 2 | 1 | 8 |
| Surge JSON | 0 | 0 | 9 | 9 | 0 | 7 | 0 | 7 |
| PSLF .epc | 0 | 0 | 1 | 1 | 0 | 4 | 3 | 0 |

`DEEPMIND_OPFDATA_WARNING_BASELINE` (line 724), one source row over the same eight targets: 3, 2, 5, 5, 3, 4, 2, 4.

`DISTRIBUTION_WARNING_BASELINE` (line 980), sources and targets in the order OpenDSS .dss, BMOPF JSON, PMD JSON:

| source / target | dss | BMOPF | PMD |
|---|---|---|---|
| OpenDSS .dss | 0 | 66 | 88 |
| BMOPF JSON | 20 | 0 | 15 |
| PMD JSON | 26 | 42 | 0 |

Cases: `case9.m`, `case14.m`, `case30.m`, `t_case9_dcline.m`, `t_case9_oos.m`, `pglib/pglib_opf_case5_pjm.m` (lines 726-733); `opfdataset/example_0.json`; and seven distribution cases (lines 982-1018). What the six MATPOWER cases carry, read from the files: every case has `mpc.gencost` (all polynomial except `t_case9_dcline`, which has two piecewise and one polynomial row); all but PGLib 5 have 21-column gen rows (capability and ramp columns stated as zeros); `case14` has `mpc.bus_name`, one bus shunt, three tapped branches, and base kV 0 on every bus; `case30` has two bus shunts; `t_case9_dcline` has four `mpc.dcline` rows and `mpc.dclinecost`; PGLib 5 has `mpc.areas`; `t_case9_oos` has out-of-service elements.

The comment block above the arrays (lines 601-712) attributes every count change to a warning text. The attributions it records, reduced to one line each:

- PSLF column: minus 5 on every source whose buses carry no base kV (the generator voltage setpoint now rides `vsched`).
- egret writer emits `dc_branch`, so dclines survive into egret and are dropped at the next hop instead.
- PowerModels column: the blanket dcline warning is gone; every `Hvdc` field has a PowerModels slot.
- PSLF reader: the "control fields retained in extras" remark fires only when the DC record states control detail.
- MATPOWER writer emits `mpc.dcline` and `mpc.dclinecost`; targets without a slot report the drop: PSS/E +2 (converter detail, cost), PowerWorld +1 (drop), pandapower +1 (drop), egret +1 (cost), Surge +2 (received power off the loss model, cost), PSLF +2 (asymmetric pmin, cost).
- MATPOWER writer no longer warns on a costless network omitting `mpc.gencost` (minus 6 on the PSS/E, PowerWorld, and PSLF source rows).
- PSS/E, PowerWorld, and PSLF readers retain extras only when the record states more than the writer synthesizes, so those source rows reach MATPOWER with nothing to drop.
- MATPOWER writer emits the 21-column gen row when any generator carries caps; the five cases that state caps report the drop at every target without a slot: +5 on PSS/E, PowerWorld, pandapower, PSLF; +20 on Surge (per generator); egret and PSLF writers now say so too.
- pandapower writer emits `dcline` and `res_gen`; what the table has no column for is warned (pmin floor, received power off the loss model, usage cost); one voltage setpoint per bus is coerced and warned (+1 on the MATPOWER, PowerModels, egret, Surge rows).
- pandapower reader no longer stores `max_i_ka` as a second rating; piecewise costs map both ways and reading a `pwl_cost` payload declares the unstated absolute level once per parse (+1 across the pandapower source row, twice on the diagonal).
- MATPOWER reader reads `mpc.areas` and the writer emits it: the six targets without an area table declare the drop (+1 on the MATPOWER and PSS/E source rows' PowerModels, PowerWorld, egret, pandapower, Surge, PSLF cells); the PSS/E area record carries a classification MATPOWER's `mpc.areas` cannot hold (+1 on PSS/E to MATPOWER).

### 5.2 How a counted line is built, and which code paths run

- `render_diagnostic` is `format!("{}: {}", code, message)` (`/home/sam/Research/powerio/powerio-core/src/diagnostic.rs:647`); details, target, severity, and suggested action are not rendered. The matrix builds parse text the same way (conversion_matrix_report.rs:1345) and counts Remark and Error records exactly like Warnings (line 348).
- Parse: `powerio_tx::format::parse` runs `read_source` (fmt:794-826). The MATPOWER reader (fmt:797) and the egret reader (fmt:807) take no collector and emit nothing; every other reader receives `warnings`.
- Write: `emit_text` (fmt:1041) takes the `emit_value_text` path (fmt:1084) because `PioModule::new(network.clone())` has no retained source; the shared passes at fmt:1127-1131 then run (`warn_normalized_tap`, `warn_missing_reference`, base frequency, locations, PSLF transformer charging). `warn_psse_downgrade` (fmt:1049) needs a retained source and never fires. `apply_emit_cost_policy` (fmt:1331) runs only under non-default options, so `TRANSFORM.GEN_COST.POLICY_APPLIED` never fires. No parse or emit path calls `to_normalized`, so no `CANONICALIZE.*` code fires.
- Distribution: `powerio_dist::parse`, then `emit_value_text_with_options` (`/home/sam/Research/powerio/powerio-dist/src/convert.rs:491`) runs the writer and the shared route pass.

### 5.3 Classification key

Each site below is classified as one of:

- `inherent`: the target format has no field, record, or table for the data. The sentence after the tag states the limitation.
- `avoidable`: the writer could carry or reconstruct the data. The sentence states what PowSybl's exporter does instead. PowSybl exports MATPOWER (`.mat`), PSS/E (RAW and RAWX 35), XIIDM, CGMES, AMPL, and UCTE. It has no PowerWorld, egret, pandapower, Surge, PSLF, OpenDSS, BMOPF, or PMD exporter, so for those targets the sentence cites the nearest PowSybl exporter behaviour or states that the format itself has the slot.
- `restatement`: the warning reports a substitution or a default rather than a loss.

### 5.4 Shared writer passes (fmt)

| code | message literal | fires when | class |
|---|---|---|---|
| `EMIT.<T>.FIELD_DROPPED` (fmt:1442) | `"system base frequency {} Hz dropped: {} has no frequency field (reads back as {} Hz)"` | target is not PSS/E, RAWX, or pandapower and base frequency differs from 60 Hz; once per file | inherent for MATPOWER, PowerModels, egret, Surge, PowerWorld aux, PSLF: none of those files states a system frequency. PowSybl's IIDM has no frequency either; its exporters write none. Does not fire in the matrix (every payload is 60 Hz). |
| `EMIT.<T>.FIELD_DROPPED` (fmt:1471) | `"{n} bus location(s) and {routed} branch route(s) dropped: {} has no coordinate field (...)"` | target is not PowerWorld or pandapower and any bus has a location or any branch a route | inherent: MATPOWER, PowerModels, egret, Surge, PSLF, PSS/E state no coordinates. PowSybl carries coordinates only in the `SubstationPosition` and `LinePosition` extensions, which only XIIDM exports. Does not fire in the matrix. |
| `EMIT.PSLF.FIELD_DROPPED` (fmt:1501) | `"{n} transformer(s) carry line charging that the PSLF .epc transformer record cannot represent; the charging was dropped"` | target is PSLF and a transformer branch has nonzero total charging | inherent: the PSLF transformer record states series impedance, taps, and ratings only. No PowSybl exporter for PSLF. A case14 transformer branch with nonzero b is the only trigger among the six cases. |
| `EMIT.<T>.EXTRAS_DROPPED` (fmt:1555, `warn_dropped_extras`) | `"{dropped} element(s) carry source-format passthrough fields (extras) the {target} writer does not replay; dropped"` | any element carries an extras key the target's `consumed` predicate rejects; callers: MATPOWER (every key), PSS/E (`id` and `psse_*` consumed), PSLF (`id` and `pslf_*`), PowerWorld (LoadID, ShuntID, BranchDeviceType, circuit) | avoidable in most cases: an extras key is a typed field of another format. PowSybl has no extras; every IIDM field is typed, and an exporter reads only typed fields. The fix is to type the field on the neutral model instead of retaining it, which is what the PSS/E and PowerWorld readers already did for the positional defaults (comment block, lines 640-654). |
| `EMIT.<T>.AREAS_DROPPED` (fmt:1574, `warn_dropped_areas`) | `"{} area record(s) dropped: the {target} writer emits no area table"` | `net.areas()` nonempty; callers: PowerModels, egret, pandapower, Surge, PSLF, PowerWorld | inherent for PowerModels, egret, pandapower, Surge (no area table in their schemas; the bus `area` number still survives on PowerModels and egret bus records). Avoidable for PSLF and PowerWorld aux: the EPC `area data` section and the aux `Area` object exist; the writers do not emit them. PowSybl's MATPOWER exporter writes no area table; its PSS/E exporter writes the area record. Fires once per cell for PGLib 5 on the MATPOWER and PSS/E source rows. |
| `EMIT.<T>.RATING_SET_DROPPED` (fmt:1592, `warn_extra_branch_rating_sets`) | `"branch {} ({} to {}) rating set {}={} MVA dropped: {} has no field for branch rating sets beyond rate_a, rate_b, and rate_c"` | per `BranchRatingSet` on any branch; every target except RAW 34/35 and RAWX | inherent: those targets state at most three ratings. PowSybl's MATPOWER exporter collapses IIDM temporary limits to RATE_B and RATE_C by duration (PE:374-413); its PSS/E exporter writes RATE1-RATE12. Does not fire in the matrix (no case carries rating sets). |
| `EMIT.<T>.REFERENCE_MISSING` (fmt:1628, `warn_missing_reference`) | `"no reference (slack) bus in the source network; power flow tools reject such cases; to_normalized synthesizes a slack at the largest pmax in service generator bus"` | no bus is `BusType::Ref`; targets MATPOWER, PSS/E, RAWX, PowerModels, pandapower, PSLF, Surge | avoidable: PowSybl's MATPOWER exporter picks one REF per synchronous component (SlackTerminal, else largest generator maxP, else regulating VSC count, else bus id; PE:1138-1162) and never writes a slackless case. powerio could designate the same way in the writer. Does not fire in the matrix (every case has a type 3 bus). |
| `EMIT.<T>.ELEMENT_RELABELED` (fmt:1660, `warn_normalized_tap`) | `"normalized network: {} branch(es) have unit tap and no phase shift, so the line/transformer label is not preserved (the power flow is identical)"` | target is not MATPOWER, network is normalized, some branch has tap 1.0 and shift 0.0 | restatement. Parsed networks are not normalized; never fires in the matrix. |
| `EMIT.<T>.NOT_A_NUMBER` (fmt:1791, `finish`) | `"non-finite numeric values written as JSON null in field(s): {}"` | any null in the emitted JSON tree; PowerModels, egret, Surge, pandapower | restatement: JSON has no Inf or NaN. PowSybl's exporters replace NaN with a default (PE:1003-1019, 1130-1136). Fires only when a case carries an infinite bound; none of the six does. |

### 5.5 MATPOWER writer sites (mp-wr, family EMIT_MATPOWER)

Reachable in the matrix from the PSS/E, pandapower, PSLF, Surge, egret, PowerModels, and PowerWorld source rows, all of which land on 0 or 1.

| code (line) | message literal | class |
|---|---|---|
| RECORD_DROPPED (mp-wr:55) | `"{} switch(es) dropped: MATPOWER has no switch table"` | avoidable: PowSybl's exporter merges closed retained switches into one bus view bus and splits on open ones (PE:322, 657-658), so the topology survives. powerio could write a closed switch as a minimum-impedance branch or merge its endpoints. Not triggered in the matrix. |
| RECORD_DROPPED (mp-wr:64) | `"{} 3-winding transformer(s) dropped: the canonical MATPOWER writer emits no 3-winding record (star-expand them into branches before writing to keep them)"` | avoidable: PowSybl writes a star bus plus three branches (PE:257-283, 722-783); powerio has `expand_transformers_3w` (net:3921) and does not call it. Not triggered in the matrix. |
| FIELD_DROPPED (mp-wr:78) | `"emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band"` | inherent: the MATPOWER bus row has one VMAX/VMIN pair. PowSybl's exporter writes the voltage level limits only. Triggered by the PSS/E source row when a payload states EVHI/EVLO (revision 33 output writes the normal band into both, so it does not fire there). |
| VALUE_COLLAPSED (mp-wr:89) | `"{} branch terminal admittance record(s) collapsed to total susceptance: MATPOWER cannot carry conductance or asymmetric terminal charging"` | inherent: BR_B is one total susceptance. PowSybl does the same sum and drops g1/g2 silently (PE:643-653). |
| FIELD_DROPPED (mp-wr:99) | `"{} branch current rating record(s) dropped: MATPOWER branch rows carry MVA ratings only"` | avoidable when rate_a/b/c are zero: PowSybl converts current limits to MVA through the side 1 nominal voltage (PE:393-395, 422-423). Restatement when the MVA ratings are also present. |
| VALUE_DEFAULTED (mp-wr:113) | `"{} generator(s) with no capability or ramp data written with zeros in columns 11-21: ..."` | restatement: MATPOWER's gen matrix is rectangular; PowSybl always writes 21 zero columns without comment (PW:99, 112-122). |
| RECORD_DROPPED (mp-wr:134) | `"{} out of service load(s) dropped, {:.4} MW and {:.4} MVAr: a MATPOWER bus row states one demand with no status, so an idle load would read back as live"` | inherent: PD/QD have no status. PowSybl sums connected loads only (PE:332-348). |
| RECORD_DROPPED (mp-wr:145) | `"{} out of service shunt(s) dropped: a MATPOWER bus row states one shunt with no status"` | inherent, same as the load row; PowSybl sums connected shunt compensators only (PE:349-357). |
| FIELD_DROPPED (mp-wr:157) | `"{} branch solution value set(s) dropped: MATPOWER branch rows do not carry solved flow columns"` | avoidable: MATPOWER's own `savecase` writes PF, QF, PT, QT as columns 14-17 when present; the reader could read them too. PowSybl's exporter writes 13 columns and drops them. |
| FIELD_DROPPED (mp-wr:171) | `"{} voltage dependent load model(s) dropped: MATPOWER carries only static Pd/Qd"` | inherent: the bus row states one constant-power demand. PowSybl reads P0/Q0 only (PE:334-337). |
| FIELD_DROPPED (mp-wr:181) | `"gen cost dropped: {} of {} generators carry cost data, but MATPOWER's `mpc.gencost` block is all-or-nothing"` | avoidable: a missing row can be written as a zero polynomial (`2 0 0 3 0 0 0`), which is what the `zero` cost policy does; PowSybl writes no gencost at all. Triggered on the pandapower source row when one of three generators has a cost (comment block, line 690). |
| EXTRAS_DROPPED (mp-wr:190) | see 5.4 | see 5.4 |
| FIELD_DROPPED (mp-wr:204) | `"{} of {} area record(s) carry a name, source identity, classification, or interchange data: `mpc.areas` holds only the area number and reference bus"` | inherent: `mpc.areas` has two columns. PowSybl writes no area table. Fires once on PSS/E to MATPOWER (the PSS/E reader sets `area_type` to the interchange classification). |

### 5.6 PSS/E RAW 33 writer sites (psse, family EMIT_PSSE), matrix-reachable subset

| code (line) | message literal | class |
|---|---|---|
| VALUE_DEFAULTED (psse:1240) | `"DC line converter detail (firing angles, converter transformer taps, reactive output) defaulted: PSS/E two-terminal DC is written from the power setpoint and line resistance only"` | partly inherent, partly avoidable. The two-terminal DC record has no reactive output or terminal voltage setpoint fields for a converter, so `qf/qt` and `vf/vt` cannot be stated. It does have RDC, VSCHD, SETVL, and converter taps and angles, from which `pt` and a loss model follow. PowSybl's PSS/E exporter updates an imported DC record in place and, in full export mode, writes the converter records from the LCC stations (`/home/sam/Research/powsybl-core/psse/psse-converter/src/main/java/com/powsybl/psse/converter/TwoTerminalDcConverter.java`). powerio writes from `pf`, `resistance_ohm`, and `nominal_voltage_kv` only. Fires once per dcline case on the MATPOWER, PowerModels, egret, Surge, pandapower, and PSLF source rows. |
| RECORD_DROPPED (psse:1248) | `"{} storage unit(s) dropped: PSS/E has no storage record"` | inherent: RAW has no storage record. PowSybl folds batteries into loads on MATPOWER export and has no battery record in its PSS/E model either. No case carries storage. |
| FIELD_DROPPED (psse:1257) | `"generator cost curves dropped: PSS/E .raw has no cost data"` | inherent: RAW carries no cost data. PowSybl's IIDM has no generator cost, so its exporter never faces the loss. Fires once per case (six per cell) on every source row whose payload keeps costs. |
| FIELD_DROPPED (psse:1264) | `"DC line cost curves dropped: PSS/E .raw has no cost data"` | inherent, same reason. Once per dcline case. |
| FIELD_DROPPED (psse:1270) | `"branch angle limits (angmin/angmax) dropped: PSS/E branch records carry none"` | inherent: the non-transformer branch record has no angle difference limit. PowSybl's model has a `VoltageAngleLimit` element, which its PSS/E exporter does not write either. |
| FIELD_DROPPED (psse:1288) | `"{} non-transformer branch name(s) dropped: PSS/E revision 33 branch records have no name field"` | inherent to revision 33; avoidable by writing revision 34 or 35 (the matrix pins the target to 33). PowSybl full export writes revision 35 and keeps the NAME field. |
| FIELD_DROPPED (psse:1302) | `"{} branch current rating record(s) dropped: PSS/E branch ratings are MVA ratings"` | avoidable when the MVA ratings are zero: PowSybl's PSS/E exporter converts an IIDM current limit to MVA through the nominal voltage and writes RATE1. |
| FIELD_DROPPED (psse:1373) | `"{} branch solution value set(s) dropped: PSS/E RAW power flow result fields are not written"` | inherent: RAW states no branch flows. PowSybl writes none. |
| VALUE_COLLAPSED (psse:1387) | `"{} transformer terminal admittance record(s) collapsed to magnetizing admittance: PSS/E transformer records cannot preserve terminal side assignment"` | inherent: MAG1/MAG2 sit at the winding 1 bus. PowSybl's importer does the same move in the other direction (docs/psse/import.md:185). |
| FIELD_DROPPED (psse:1392) | `"generator ramp/capability columns dropped: PSS/E .raw has no equivalent fields"` | inherent: the generator record has no ramp or capability columns. Fires once per case with caps (five per cell). |
| NOT_A_NUMBER (psse:1398) | `"non-finite values written as +/-1e10 sentinels (PSS/E has no Inf/NaN)"` (the literal uses the plus-minus character) | restatement; PowSybl replaces NaN by defaults. |
| VALUE_SUBSTITUTED (psse:1404) | `"{} quoted PSS/E field(s) contained a quote or '/' that would corrupt a record; replaced with spaces"` | inherent to the fixed text syntax. |
| FIELD_DROPPED (psse:168) | `"{} generator energy source value(s) dropped ({}): PSS/E RAW and RAWX generator records have no energy source field"` | inherent: no energy source field. PowSybl's PSS/E exporter drops `energySource` too. Not triggered in the matrix. |
| FIELD_DROPPED (psse:4358), VALUE_SUBSTITUTED (psse:4372, 4404), FIELD_DROPPED (psse:4383) | load voltage model spellings: `"... nominal voltage has no load record field; dropped"`, `"... stale voltage model components did not match typed p/q; wrote typed p/q as constant power"`, `"... exponential voltage model has no load record fields; wrote typed p/q as constant power"` | inherent for the nominal voltage and the exponential model (the load record states PL/QL/IP/IQ/YP/YQ only). PowSybl's importer keeps only the constant power part (docs/psse/import.md:66). |
| EXTRAS_DROPPED (psse:1360) | see 5.4 | see 5.4 |
| RATING_SET_DROPPED (psse:139, revision 33 only) | see 5.4 | see 5.4 |

Not reachable with revision 33 as the target: psse:101/111 (rating set remap, revisions 34 and 35), psse:399 (load type needs revision 35), psse:483/526 (NREG, BASLOD), psse:702 (switching devices), psse:755/912/965 (ZCOD, COD 4), psse:1203-1231 (substation data), psse:1313-1345 (switch ratings), psse:1553-1646 (retained property parsing and unresolvable regulating terminals).

### 5.7 PowerWorld aux writer sites (`/home/sam/Research/powerio/powerio-tx/src/format/powerworld/map.rs`, family EMIT_POWERWORLD)

| code (line) | message literal | class |
|---|---|---|
| FIELD_DROPPED (900) | `"generator cost curves dropped: not written to PowerWorld .aux"` | avoidable: PowerWorld aux has a `GenCost` or `Gen` cost model object family (`GenCostModel`, `GenBidCurve`), but no vendored export states its field names, so the writer refuses to invent them (file header comment, conversion_matrix_report.rs:76-81). No PowSybl exporter. |
| RECORD_DROPPED (906) | `"{} dcline(s) dropped: PowerWorld HVDC not modeled"` | avoidable in principle (aux has a `DCTransmissionLine` object); same vendored-vocabulary reason. |
| RECORD_DROPPED (915) | `"{} 3-winding transformer(s) dropped: the PowerWorld .aux writer emits no 3-winding record"` | avoidable: the aux format states three winding transformers as three `Branch` rows and a star bus; PowSybl's MATPOWER exporter star-expands. |
| FIELD_DROPPED (925) | `"emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band"` | inherent to the bus fields the writer emits. |
| RECORD_DROPPED (931) | `"{} storage unit(s) dropped: PowerWorld storage not modeled"` | avoidable (aux has energy storage objects); same vocabulary reason. |
| FIELD_DROPPED (949) | `"{} voltage dependent load model(s) dropped: PowerWorld Load records carry static MW/MVR only"` | inherent for the Load object as written (aux has `LoadModelGroup` objects the writer does not emit). |
| VALUE_COLLAPSED (959) | `"{} branch terminal admittance record(s) collapsed to total susceptance: ..."` | inherent for the branch fields emitted. |
| FIELD_DROPPED (969) | `"{} branch current rating record(s) dropped: PowerWorld aux branch rows written here carry MVA ratings only"` | avoidable when MVA ratings are zero (convert through base kV). |
| FIELD_DROPPED (992) | `"{} branch solution value set(s) dropped: PowerWorld aux result fields are not written"` | avoidable: aux carries `LineMW`, `LineMvar` fields; the writer does not emit them. |
| FIELD_DROPPED (997) | `"branch angle limits (angmin/angmax) dropped: not written to PowerWorld .aux"` | inherent: no angle limit field on a PowerWorld branch. |
| FIELD_DROPPED (1003) | `"generator ramp/capability columns dropped: not written to PowerWorld .aux"` | avoidable for ramp rates (aux `Gen` has `GenRampRate` style fields), inherent for the Pc/Qc capability points. Fires once per case with caps. |
| NOT_A_NUMBER (1009), VALUE_SUBSTITUTED (1015) | sentinels and quote sanitizing | restatement. |
| shared rating sets (973), extras (978), areas (985) | see 5.4 | see 5.4 |

### 5.8 egret writer sites (`/home/sam/Research/powerio/powerio-tx/src/format/egret.rs`, family EMIT_EGRET)

| code (line) | message literal | class |
|---|---|---|
| FIELD_DROPPED (59) | `"generator capability/ramp columns dropped for {} generator(s): the egret generator records written here carry none"` | avoidable: egret's generator dictionary has `ramp_up_60min`, `ramp_down_60min`, `startup_capacity`, `shutdown_capacity`, and `p_min`/`p_max` fields the writer could fill from RAMP_10/RAMP_30; inherent for the Pc/Qc points. Fires once per case with caps. |
| REFERENCE_MISSING (86) | `"no single reference bus (BusType::Ref); system.reference_bus omitted"` | avoidable, see the shared reference row. |
| RECORD_DROPPED (102) | `"{} 3-winding transformer(s) dropped: the egret writer emits no 3-winding record"` | avoidable by star expansion. |
| FIELD_DROPPED (115) | `"emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band"` | inherent for egret bus records. |
| RECORD_DROPPED (121) | `"{} storage unit(s) dropped: egret storage mapping not implemented"` | avoidable: egret's ModelData has a `storage` element family. |
| FIELD_DROPPED (139) | `"{} voltage dependent load model(s) dropped: egret load records carry static p_load/q_load only"` | inherent. |
| VALUE_COLLAPSED (149) | `"{} branch terminal admittance record(s) collapsed to total susceptance: egret branches cannot carry conductance or asymmetric terminal charging"` | inherent. |
| FIELD_DROPPED (159) | `"{} branch current rating record(s) dropped: egret branch records carry MVA ratings only"` | avoidable when MVA ratings are zero. |
| FIELD_DROPPED (170) | `"{} branch solution value set(s) dropped: egret branch result fields are not written"` | avoidable: egret branches carry `pf`, `pt`, `qf`, `qt` result fields. |
| FIELD_DROPPED (293) | `"generator at bus {} has a cost model egret's writer can't express; cost dropped"` | inherent for models other than polynomial and piecewise. |
| FIELD_DROPPED (308) | `"dcline {} -> {} cost curve dropped: egret dc_branch records carry no cost"` | inherent. Once per dcline. |
| shared areas (100), rating sets (163), not a number (96) | see 5.4 | see 5.4 |

### 5.9 pandapower writer sites (`/home/sam/Research/powerio/powerio-tx/src/format/pandapower.rs`, family EMIT_PANDAPOWER), matrix-reachable subset

| code (line) | message literal | class |
|---|---|---|
| RECORD_DROPPED (939) | `"{} 3-winding transformer(s) dropped: the pandapower JSON writer emits no trafo3w table"` | avoidable: pandapower has a `trafo3w` table. |
| FIELD_DROPPED (949) | `"emergency voltage band(s) (EVHI/EVLO) dropped: this writer carries one voltage band"` | inherent for the `bus` table. |
| RECORD_DROPPED (955) | `"{} storage unit(s) dropped: the pandapower JSON writer does not model storage"` | avoidable: pandapower has a `storage` table. |
| VALUE_DEFAULTED (967) | `"{} bus(es) carry no base_kv; written with vn_kv = 1 so pandapower's ohm-based model stays defined (per-unit impedances are preserved exactly)"` | restatement forced by pandapower's ohm-based model; PowSybl's MATPOWER importer does the same (nominalV 1 when base kV is 0, PI:255-257). Fires for case14 on every source row whose payload keeps base kV 0. |
| FIELD_DROPPED (980) | `"generator capability/ramp columns dropped for {} generator(s): pandapower gen tables have no MATPOWER capability columns"` | inherent for the `gen` table (pandapower keeps no ramp columns). Once per case with caps. |
| FIELD_DROPPED (991) | `"{} branch angle limit(s) dropped: pandapower line/trafo tables do not carry MATPOWER angle limits"` | inherent. |
| FIELD_DROPPED (999) | `"{} branch rate_b/rate_c value set(s) dropped: pandapower carries one loading limit"` | inherent: `max_i_ka` is one rating. Fires when rate_b or rate_c differ from rate_a (case9, case30, dcline, oos, PGLib 5 state them). |
| FIELD_DROPPED (1009) | `"{} branch current rating record(s) dropped: pandapower line/trafo tables carry MVA loading limits, not current ratings"` | avoidable: `max_i_ka` is itself a current rating; a current rating could be written directly. |
| FIELD_DROPPED (1021) | `"{} branch solution value set(s) dropped: pandapower branch result tables are not written"` | avoidable: `res_line` and `res_trafo` exist; the writer already emits `res_gen`. |
| VALUE_COLLAPSED (1029) | `"{} transformer terminal charging shunt(s) written into `shunt`: pandapower's trafo magnetizing model is inductive only, so MATPOWER transformer line charging b rides as bus shunts (Y_bus exact)"` | restatement forced by the trafo model; the admittance matrix is exact. |
| VALUE_SUBSTITUTED (1155, 1211), FIELD_DROPPED (1162, 1168), VALUE_SUBSTITUTED (1184, 1123) | load voltage model spellings | inherent for exponential models and nominal voltages (pandapower `load` has `const_z_percent` and `const_i_percent` only). |
| VALUE_COLLAPSED (1493) | `"branch {} -> {} terminal admittance collapsed to symmetric line charging: pandapower line tables cannot carry asymmetric terminal charging"` | inherent. |
| VALUE_SUBSTITUTED (1528) | `"{} series capacitive transformer branch(es) (x < 0) written with a negative vk_percent: ..."` | inherent to pandapower's `vk_percent` magnitude. |
| VALUE_SUBSTITUTED (1623) | `"{} dcline terminal voltage setpoint(s) coerced to the bus's controlling setpoint: pandapower enforces one voltage setpoint per bus"` | inherent to pandapower's one-setpoint-per-bus rule. Fires on the dcline case where vf 1.01 disagrees with a generator vg 1.0. |
| FIELD_DROPPED (1629) | `"{} dcline sending power floor(s) (pmin) dropped: pandapower dcline caps max_p_mw only"` | inherent: `dcline` has `max_p_mw` only. |
| FIELD_DROPPED (1639) | `"{} dcline reactive flow value pair(s) (qf/qt) dropped: pandapower dcline states limits, not flows"` | avoidable in part: `res_dcline` carries `q_from_mvar`/`q_to_mvar`; the writer could emit it as it does `res_gen`. |
| FIELD_DROPPED (1649) | `"{} dcline received power value(s) dropped: pandapower dcline derives the receiving end from loss_mw/loss_percent, which disagree with the stated pt"` | inherent when `pt` is off the loss model; restatement otherwise. |
| FIELD_DROPPED (1654) | `"DC line cost curves dropped: pandapower dcline carries no cost data"` | inherent (pandapower costs attach to gen, sgen, ext_grid, storage, dcline is not a cost element type). |
| FIELD_DROPPED (1750), VALUE_TRUNCATED (1755), VALUE_DEFAULTED (1760) | generator cost: unsupported models, cubic or higher truncated to quadratic, empty coefficient sets written as zero | inherent: `poly_cost` has cp0, cp1, cp2 only. |
| FIELD_DROPPED (1813, 1818) | piecewise curves with a nonzero starting cost (absolute level lost) or fewer than two breakpoints | inherent: `pwl_cost` stores marginal cost per range. Fires on the dcline case. |
| NOT_A_NUMBER (1860) | `"`{table}`: non-finite value(s) written as null in column(s) {}; pandapower reads them as NaN"` | restatement. |
| shared rating sets (1013), areas (1014) | see 5.4 | see 5.4 |

### 5.10 Surge writer sites (`/home/sam/Research/powerio/powerio-tx/src/format/surge.rs`, family EMIT_SURGE)

| code (line) | message literal | class |
|---|---|---|
| FIELD_DROPPED (293) | `"generator at bus {} has MATPOWER capability or ramp columns not represented in Surge JSON"` | inherent for the Surge generator body the writer emits; Surge's schema carries ramp fields in a richer generator record the reader retains only in source text (surge.rs:1155), so this is avoidable in part by writing those fields. Fires once per generator with caps (20 per cell on the MATPOWER and PowerModels rows). |
| FIELD_DROPPED (320, 340) | odd piecewise coefficient count; unsupported cost model | inherent for models other than 1 and 2. |
| FIELD_DROPPED (445) | `"dcline {} terminal reactive flow, received power, or cost dropped: a Surge link states the scheduled setpoint and the loss model only"` | inherent: a Surge link states no terminal reactive flow, no cost, and no received power. Fires on the dcline case. |
| shared rating sets (94), areas (95), not a number (96) | see 5.4 | see 5.4 |

### 5.11 PSLF writer sites (`/home/sam/Research/powerio/powerio-tx/src/format/pslf.rs`, family EMIT_PSLF)

| code (line) | message literal | class |
|---|---|---|
| VALUE_SUBSTITUTED (1575) | `"PSLF generator at bus {}: voltage setpoint {} p.u. could not be written because bus base kV is missing and the bus schedules a different setpoint"` | inherent to the EPC generator record, which states `reg_kv` in kV; the writer already moves the setpoint to the bus `vsched` when the bus has one. |
| FIELD_DROPPED (1665) | `"{} HVDC line(s) have asymmetric power limits (pmin != -pmax); the PSLF .epc dc record carries only rate1 (= pmax), so pmin reads back as -pmax"` | inherent. Fires on the dcline case. |
| RECORD_DROPPED (1674) | `"{} storage unit(s) dropped: PSLF .epc has no storage record"` | inherent. |
| FIELD_DROPPED (1683) | `"generator cost curves dropped: PSLF .epc carries no cost data"` | inherent. Once per case. |
| FIELD_DROPPED (1690) | `"generator capability/ramp columns dropped for {} generator(s): the PSLF .epc generator records written here carry no MATPOWER capability columns"` | inherent. Once per case with caps. |
| FIELD_DROPPED (1695) | `"DC line cost curves dropped: PSLF .epc carries no cost data"` | inherent. |
| VALUE_COLLAPSED (1709) | terminal admittance collapsed on lines | inherent. |
| FIELD_DROPPED (1723) | `"{} transformer charging admittance record(s) dropped: PSLF transformer records written here carry series impedance, tap, shift, and ratings only"` | inherent; see fmt:1501. |
| FIELD_DROPPED (1733) | current ratings dropped | avoidable when MVA ratings are zero. |
| FIELD_DROPPED (1773) | branch solutions dropped | inherent: EPC states no flows. |
| FIELD_DROPPED (1785) | `"{} generator(s) lost their remote regulated bus: the PSLF .epc generator record this writer emits controls the unit's own terminal"` | avoidable: the EPC generator record has a `reg_bus` field; the writer does not emit it. PowSybl's MATPOWER exporter localizes and logs (PE:854-857); its PSS/E exporter writes IREG. |
| FIELD_DROPPED (1798) | `"PSLF 3-winding export carries the primary winding ratio/ratings only; secondary/tertiary winding ratios/ratings dropped"` | avoidable: the EPC transformer record states all three windings. |
| FIELD_DROPPED (1812) | `"{} transformer(s) lost their regulating control (mode/tap limits/regulated bus): the PSLF .epc transformer record carries no control columns"` | avoidable in part: the EPC transformer record has control fields (`reg_bus`, `vmax`, `vmin`, `tmax`, `tmin`); the writer does not emit them. |
| FIELD_DROPPED (1824) | `"{} switched shunt(s) written as fixed: the PSLF .epc shunt record this writer emits has no switching-control columns (mode/band/step blocks)"` | avoidable: EPC has an `svd data` section, which the reader reads (pslf.rs:853). |
| VALUE_SUBSTITUTED (1834), NOT_A_NUMBER (1843) | quote sanitizing and sentinels | restatement. |
| FIELD_DROPPED (1953, 1962, 1983), VALUE_SUBSTITUTED (1976) | load voltage model spellings | inherent for exponential models and nominal voltage; avoidable for the PSS/E load type and scaling only by extending the EPC load record, which has no such field, so inherent. |
| shared rating sets (1737), extras (1742), areas (1766) | see 5.4; areas are avoidable here (EPC has an `area data` section) | |

### 5.12 PowerModels writer sites (`/home/sam/Research/powerio/powerio-tx/src/format/powermodels.rs`, family EMIT_POWERMODELS)

| code (line) | message literal | class |
|---|---|---|
| VALUE_COLLAPSED (96) | `"{} storage unit(s) mapped with warnings to the PowerModels storage schema"` | restatement. |
| RECORD_DROPPED (105) | 3-winding transformers dropped | avoidable by star expansion. |
| FIELD_DROPPED (121) | voltage dependent load models dropped | inherent for PowerModels `load` (pd, qd only). |
| FIELD_DROPPED (131) | EVHI/EVLO dropped | inherent. |
| shared areas (110), rating sets (125), not a number (152) | see 5.4 | see 5.4 |

### 5.13 Reader sites counted as source parse or target readback

| reader, code (line) | message literal | class |
|---|---|---|
| PowerModels, READ.POWERMODELS.RECORD_DROPPED (476) | `"multinetwork=true: only the top-level single snapshot was read"` | inherent to the balanced model (one snapshot). |
| PowerModels, READ.POWERMODELS.FIELD_DROPPED (788) | `"{} branch(es) carry an off-nominal `tap` without `transformer: true`, so the tap is discarded ..."` | restatement of a PowerModels rule. |
| PSS/E, READ.PSSE.RETAINED_SOURCE_ONLY (3167) | `"PSS/E load at bus {} id {:?}: interruptible/DG/flag fields are retained in extras"` | avoidable: PowSybl's PSS/E importer keeps every load field in its `PsseLoad` model for update-mode export; typing INTRPT, DGENP, DGENQ, DGENM on `LoadVoltageModel` or `Load` would remove the remark. |
| PSS/E, READ.PSSE.REFERENCE_DROPPED (2520-2590) | `"... references missing bus id {}; dropped ..."` for IREG, CONT, SWREM, ISW | both drop; PowSybl's `PsseValidation` rejects the case instead (`/home/sam/Research/powsybl-core/psse/psse-model/src/main/java/com/powsybl/psse/model/pf/PsseValidation.java`). |
| PSS/E, READ.PSSE.SECTION_UNSUPPORTED (2502) | `"PSS/E {} section ({} record line(s)) is not modeled: preserved only in a same-format .raw ..., dropped on any other write"` (one word elided from the literal) | see the PSS/E section for which sections PowSybl models. Not triggered by powerio's own output. |
| PSS/E, READ.PSSE.VALUE_SUBSTITUTED (4122, 4133) | DC record with a non-finite drop model, or a scheduled current with no scheduled voltage | restatement. |
| PowerWorld, READ.POWERWORLD.RETAINED_SOURCE_ONLY (map.rs:116) | `"PowerWorld .aux DATA {} has {} row(s) not modeled in BalancedNetwork; retained only in source text for same-format writeback"` | inherent to the model for objects like `Area`, `Zone`, `Owner`, `Interface`; avoidable for `Area` and `Zone` (the model has `Area` and `Bus.zone`). |
| pandapower, READ.PANDAPOWER.VALUE_INFERRED (2319) | `"`pwl_cost`: {} piecewise curve(s) read; pandapower stores marginal cost per range only, so breakpoint costs start at zero at the first breakpoint and the absolute objective level is unstated"` | inherent to `pwl_cost`. Once per parse of a piecewise payload (the dcline case). |
| pandapower, READ.PANDAPOWER.TABLE_UNSUPPORTED (870, 768) | `"`{}` table ignored ({} rows): {}"` for trafo3w, ward, xward, impedance, motor, switch, and every unmapped nonempty table including `res_*` | avoidable for `trafo3w` (the model has `Transformer3W`), `switch` (the model has `Switch`), and `res_line`/`res_trafo` (the model has `Branch.solution`); inherent for ward, xward, impedance, motor. |
| pandapower, READ.PANDAPOWER.FIELD_DROPPED (261, 612, 2201, 2324, 2334) | both cost kinds on one element; tabular tap changers; reactive cost coefficients; reactive pwl rows; malformed pwl rows | avoidable for tabular tap changers (the model has `TapChanger` tables in `DetailedConnectivity`); the reactive cost rows would need the `Generator.reactive_cost` field named in section 1.1. |
| Surge, READ.SURGE.RETAINED_SOURCE_ONLY (1096-1226) | dispatch and solution profiles; unmapped sections; load, branch, generator, and HVDC fields retained only in source text | inherent to the balanced model for profiles and markets; avoidable for branch control and phase shifter bounds (the model has `TransformerControl`). |
| PSLF, READ.PSLF.VALUE_DEFAULTED (88, 102, 188, 730) | jump threshold; 3W secondary and tertiary ratios defaulted; no sbase; reg_kv with no base kV | 102 is avoidable (the EPC record states all windings); 188 is inherent to a file with no `sbase` parameter. |
| PSLF, READ.PSLF.RETAINED_SOURCE_ONLY (777, 853, 1038, 1069) | ZIP components folded; svd reduced to fixed shunts; DC control fields in extras; unmodeled sections | 777 is a restatement (the typed load voltage model keeps the components); 853 is avoidable (`SwitchedShuntControl` exists on `Shunt`); 1038 fires only when the record states control detail. |
| PSLF, READ.PSLF.RECORD_DROPPED and SOURCE_MALFORMED (278, 320, 336, 344, 948, 1045) | stray lines, count mismatches, duplicate sections, missing end marker, unmappable DC records | parse checks on malformed input; not triggered by powerio's own output. |
| OPFData, READ.OPFDATA.VALUE_INFERRED (863) | `"OPFData does not carry original bus IDs/names, areas/zones, or base frequency; synthesized IDs 1..{}, area/zone 1, and {} Hz"` | inherent to OPFData. Fires on every parse (once on the OPFData row). |
| OPFData, READ.OPFDATA.RETAINED_SOURCE_ONLY (858) | generator initial values carried in the parsed solution | restatement. Once per parse. |
| OPFData, READ.OPFDATA.FIELD_DROPPED (531, 868) | unknown schema fields; objective mismatch | inherent for unknown fields. |

### 5.14 Distribution sites

The dss, BMOPF, and PMD sites number about 250; the enumeration above covers the transmission matrix that section 1 through 4 compare against PowSybl. PowSybl has no distribution exporter, so no site here has a PowSybl counterpart. The counts in `DISTRIBUTION_WARNING_BASELINE` are attributed by the comment block at conversion_matrix_report.rs:958-979:

- dss to BMOPF (66) and dss to PMD (88): each single-phase `Load` part restates the `kv`, `phases`, and `conn` extras the dss reader attaches, dropped again on the way in (`EMIT.BMOPF.FIELD_DROPPED` aggregated per key at `/home/sam/Research/powerio/powerio-dist/src/bmopf/write.rs:337`; `EMIT.PMD.FIELD_DROPPED` at `/home/sam/Research/powerio/powerio-dist/src/pmd/write.rs:285`); avoidable by typing `kv`, `phases`, and `conn` on the multiconductor load.
- BMOPF to dss (20): named terminals and generator cost, which dss states nowhere (`EMIT.DSS.FIELD_DROPPED` at dss/write.rs:2267 for cost; `EMIT.DSS.VALUE_SUBSTITUTED` at dss/write.rs:670 for non-numeric terminals); inherent to OpenDSS. Bus voltage bounds (`EMIT.DSS.FIELD_DROPPED` at dss/write.rs:965); inherent, OpenDSS has no per-bus voltage bound.
- BMOPF to PMD (15): named terminals (`EMIT.PMD.VALUE_SUBSTITUTED` at pmd/write.rs:133) and generator cost (pmd/write.rs:776); inherent to the ENGINEERING schema.
- PMD to dss (26): the 12 bus voltage bounds plus terminal and extras drops; inherent.
- PMD to BMOPF (42): extras with no BMOPF slot; partly avoidable by typing.

The complete per-site listing for the three distribution readers and writers, with line numbers, is in the enumeration the later worker should regenerate from `grep -n "warn(\|record(\|push(" powerio-dist/src/{dss,bmopf,pmd}/*.rs`; the codes are all declared in `/home/sam/Research/powerio/powerio-dist/src/diagnostics.rs:21-236`.

### 5.15 Summary: which matrix warnings a writer could remove

Avoidable, in the order that removes the most baseline counts:

1. PSS/E `VALUE_DEFAULTED` at psse:1240: write RDC, VSCHD, SETVL and the converter records from `Hvdc` so `pt` and the loss model survive; PowSybl writes the converter records from its LCC stations. Also the `pf`/`pt` relation could be checked before warning.
2. Remote regulation and transformer control drops on PSLF (1785, 1812) and switched shunt controls (1824): the EPC records have the fields.
3. Three winding transformers on MATPOWER, PowerModels, egret, pandapower, PowerWorld: star expansion exists in `network.rs`.
4. Current ratings on every MVA-only target: convert through base kV when the MVA rating is zero, as PowSybl does on MATPOWER export.
5. Branch solution values on egret, pandapower, PowerWorld: each has a result field or table.
6. Areas on PSLF and PowerWorld: both formats have an area record.
7. The `mpc.gencost` all-or-nothing drop on MATPOWER: write a zero polynomial for the costless rows.
8. Extras drops: type the retained fields instead.

Inherent, and correctly reported today: generator and DC cost on PSS/E, PSLF, PowerWorld; capability and ramp columns on PSS/E, PSLF, pandapower; angle limits on PSS/E, PowerWorld, pandapower; storage on PSS/E, PSLF; EVHI/EVLO everywhere but PSS/E; asymmetric terminal charging on every one-susceptance format; voltage dependent load models on every constant-power format; Surge link reactive flow, received power, and cost; pandapower's one-rating, one-setpoint-per-bus, marginal-cost-only rules; pandapower `dcline` pmin and cost; PSLF dc `rate1` symmetry.
