# XIIDM gaps against PowSybl iidm-serde (actionable subset)

Line numbers refer to powerio-tx/src/format/xiidm.rs (X), powerio-tx/src/format/mod.rs (M),
powerio-tx/src/network.rs (N), powerio-tx/src/normalize.rs (Z) at branch agent/cli-composability,
and /home/sam/Research/powsybl-core/iidm/iidm-serde/src/main/java/com/powsybl/iidm/serde (S, E = S/extensions).

## Two defects

1. SlackTerminal is never read. The arm at X:1433 (`"slackTerminal" => self.slack_terminal = ...`) is
   unreachable: every child of `<iidm:extension>` other than `activePowerControl` is skipped at X:604-623 and
   X:645-663 with READ_XIIDM_ELEMENT_UNMAPPED, and `is_xiidm_namespace_uri` (X:158-170) rejects the
   `ext/slack_terminal/1_x` namespace. So X:3072-3081 never runs and every read ends at X:3091-3103, which
   picks the first in-service generator bus (else bus 1) as Ref and reports "XIIDM declares no mapped slack
   terminal". The writer has no slackTerminal output (only extension writer: X:6881-7012). No test covers a
   slackTerminal fixture. Fix (medium): recognize the `ext/slack_terminal/` namespace at the extension dispatch
   like X:112-127 and X:602-603 (E/SlackTerminalSerDe.java:25-39, 65), resolve with `resolve_regulating_bus`
   X:5874-5902; writer: emit the extension for the Ref bus's voltage level (E/SlackTerminalSerDe.java:60).
2. A VSC converter station reactive capability curve is collapsed on export. The reader keeps the curve
   (X:1159-1215, X:5479-5490) but `write_hvdc_converter_stations` always emits minMaxReactiveLimits from
   qminf/qmaxf (X:9697-9703) and never consults `index.reactive_limits` (X:6531-6536), unlike generators
   (X:8985-8993) and batteries (X:9033-9041). Fix (small).

## Small fixes

| item | PowSybl | powerio | fix |
|---|---|---|---|
| network@forecastDistance | defaults 0 when absent (S/NetworkSerDe.java:1077) | X:1572 `required_i32` rejects | make optional, default 0 |
| bus type PV vs PQ | PV means generator@voltageRegulatorOn (S/GeneratorSerDe.java:68) | X:3082-3090 marks every generator bus Pv regardless | check voltage_regulation_on |
| calculated bus property | S/VoltageLevelSerDe.java:404-405 | X:1981-1992 dropped | keep as metadata property |
| generator without reactive limits | model default | X:2741-2748 qmin=qmax=qg with VALUE_DEFAULTED | keep; message names the limitation |
| battery minP>0 or maxP<0 | S/BatterySerDe.java:58-63 | X:2793-2808 sign lost | carry signed limits |
| load/shunt/tap model property (1.16) | S/LoadSerDe.java:112,126; AbstractShuntCompensatorSerDe.java:244-278; AbstractTransformerSerDe.java:179-180 | X:1981-1992 dropped | keep as property |
| shunt targetV on non-regulating fixed shunt | S/AbstractShuntCompensatorSerDe.java:82-84 | X:2887-2925 control created only when regulating; writer X:9118-9127 same | carry targetV/targetDeadband regardless |
| SVC voltageSetpoint/reactivePowerSetpoint absent | NaN (S/StaticVarCompensatorSerDe.java:110-111) | X:2968-2971 becomes 0; writer X:9238-9271 always writes setpoints | Option, omit when absent |
| loading limit / temporaryLimit property (1.16) | S/ConnectableSerDeUtil.java:370,377 | X:1870-1882 dropped | keep as property |
| area interchangeTarget absent | S/AreaSerDe.java:59-62 | X:3518 becomes 0 | Option |
| base frequency | IIDM has none | N:1143, N:1498 stays 60 Hz silently on read | remark on read |
| terminalRef outside a tap changer | n/a | X:1446-1449 silently ignored | diagnostic |
| three winding transformer partial connection | model | X:4891-4903 out of service when not all three connected | keep; diagnostic already states it |
| tap changer regulating when loadTapChangingCapabilities=false | writer omits `regulating` (S/AbstractTransformerSerDe.java:108-109, 219-220) | X:9356, 9365 always writes | omit |
| non-finite numbers on export | PowSybl omits NaN attributes | X:10464-10472 prints NaN/inf | omit or error |
| Bus zone, evhi/evlo, element extras on export | IIDM has none | silent; M:1555-1561 `warn_dropped_extras` not called from X | call it |
| VoltagePerReactivePowerControl slope (SVC) | E/VoltagePerReactivePowerControlSerDe.java:26-37 | skipped | small |
| entsoeCategory, manualFrequencyRestorationReserve | E/GeneratorEntsoeCategorySerDe.java, E/ManualFrequencyRestorationReserveSerDe.java | skipped | small |

## Medium fixes (one struct field threaded through)

bus fictitiousP0/Q0 and node `inj` (S/BusSerDe.java:58-60, VoltageLevelSerDe.java:428-431; X:2416-2433);
generator isCondenser and equivalentLocalTargetV (X:2442-2459, never written); load loadType (X:2462-2474,
writer always UNDEFINED X:8899, 8910); load ZIP coefficients stored as MW products so zero p0 loses them
(X:2664-2709; store the coefficients); shunt solvedSectionCount (X:2477-2485); 2WT ratedS and 3WT ratedS1..3
(X:2488-2510, never written); 2WT ratedU2 != terminal nominal V normalized on output (X:4461-4517); 3WT
g2/b2/g3/b3 summed at the star point (X:4915-4930; writer always 0 at X:9446); switch kind from the balanced
table always BREAKER (X:6182-6200; needs SwitchKind on Switch); transformer not inside one substation emitted
as a line (X:8348-8363) and 3WT dropped (X:8366-8386); bus location and branch route dropped where PowSybl has
SubstationPosition/LinePosition extensions (M:1460-1480; E/SubstationPositionSerDe.java, E/LinePositionSerDe.java);
extensions VoltageRegulation (battery), LoadDetail, GeneratorStartup, CoordinatedReactiveControl,
RemoteReactivePowerControl, HvdcAngleDroopActivePowerControl, HvdcOperatorActivePowerRange, StandbyAutomaton,
OperatingStatus, BusbarSectionPosition, ReferencePriorities, ReferenceTerminals, PhaseAngleClock,
ToBeEstimated, ShortCircuit (all skipped at X:604-623).

## Large (new model concept; defer with a documented diagnostic)

IIDM 1.0 to 1.11 input (separate PR agent/iidm-versions); export to older versions; topology level export
option; areaBoundary (X:992-1003); ground, overloadManagementSystem, voltageAngleLimit (X:1456-1459);
ConnectablePosition, SecondaryVoltageControl, Measurements, DiscreteMeasurements, Observability, Fortescue,
LineCouplings, LoadAsymmetrical, ObservabilityArea extensions; import/export option sets
(S/AbstractTreeDataImporter.java:49-88, S/AbstractTreeDataExporter.java:113-179).

## Dead code

X:1451-1454 handles `selectedOperationalLimitsGroup` and `permanentLimit` element names that exist in no
version 1.12 to 1.17.

## Reader diagnostics that already name a format limitation correctly (keep)

READ_XIIDM_VALUE_DEFAULTED "XIIDM has no system MVA base; the balanced calculation view uses 100 MVA"
(X:3104-3107); READ_XIIDM_CALCULATION_VIEW for unpaired boundary lines and tie line assignments
(X:3309-3314, X:3389-3394); EMIT_XIIDM.field_dropped for generator cost (X:8996-8999), HVDC cost
(X:9766-9769), area swing bus and tolerance (X:8490-8499), base frequency (M:1433-1452), angle bounds
(X:10216-10219): IIDM has no field for any of these.
