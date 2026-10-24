# Distribution CIM fixtures

- `micro_feeder.xml` is original to this repository: a hand-written minimal
  distribution CIM (GridAPPS-D style, CIM100 namespace, format `cim-dist`) feeder
  with hand-computable expected values, pinned value-by-value in
  `powerio-dist/tests/cim.rs`. It exercises the mapped element set:
  `EnergySource`, `ACLineSegment` with `ACLineSegmentPhase` and a
  `PerLengthPhaseImpedance` matrix, a wye `EnergyConsumer` with per-phase
  `EnergyConsumerPhase`, a `LinearShuntCompensator` with `ShuntCompensatorPhase`,
  a `LoadBreakSwitch`, and a two-winding delta/wye `PowerTransformer` with
  `PowerTransformerEnd` records.

  | quantity | fixture value | expected in `MulticonductorNetwork` |
  |---|---|---|
  | connectivity nodes | sourcebus, n2, n3, n4 | 4 buses; n4 carries a grounded neutral terminal `4` |
  | line impedance | `PerLengthPhaseImpedance`, self r 0.001 / x 0.002 Ω/m, mutual r 0.0004 / x 0.0009, self b 1e-8 S/m | `DistLineCode` ohm/m: `r_series[0][0]=0.001`, `r_series[1][0]=0.0004`, `b_from[0][0]=0.5e-8` |
  | line length | `Conductor.length` 100 m | `length = 100` |
  | wye load | phaseConnection Y, per-phase p 1000 W / q 500 var | `p_nom=[1000;3]`, `q_nom=[500;3]`, terminal map `1,2,3,4` |
  | shunt | per-phase bPerSection 0.0001 S | diagonal siemens `b[0][0]=0.0001` |
  | switch | `LoadBreakSwitch` open false | closed switch n2→n3 |
  | transformer | 4160 V(Δ) / 480 V(Y), 500 kVA, end1 r 0.5 Ω x 2 Ω | `r_pct = 0.5 / (4160²/500000) × 100 ≈ 1.4446`, `xsc_pct ≈ 5.7785` |
  | source | `EnergySource.voltageMagnitude` 4160 V (line-to-line) | `v_magnitude = 4160/√3 ≈ 2401.78` V on each phase and 0 on the neutral, 120° phase spacing |

- `tank_feeder.xml` is original to this repository: a hand-written synthetic
  feeder (CIM100 namespace, format `cim-dist`) of transformers in the
  `TransformerTank` form, the form GridAPPS-D and CIMHub feeders use, pinned
  in `powerio-dist/tests/cim.rs` (`tank_feeder_maps_exactly`). Each part
  mirrors an OpenDSS micro fixture in `../micro`, and
  `tank_and_dss_fixtures_agree` checks that both read to the same
  transformer. Catalog values use the GridAPPS-D conventions: `leakageImpedance`
  is the pair reactance in ohm on the energised end,
  `x% = 100 · leakageImpedance / (ratedU² / basePower)`, and the pair
  resistance is `r% = 100 · 1000 · loss / basePower` with `loss` in kW. Each
  test's loss equals the sum of its two end resistances.

  | part | fixture value | expected in `MulticonductorNetwork` |
  |---|---|---|
  | center tap `ct_tank` (mirrors `xfmr_center_tap.dss`) | 25 kVA, 7200/120/120 V, ends `A`, `s1`, `s2`; end `r` 12.4416, 0.006912, 0.006912 Ω (0.6%, 1.2%, 1.2% on 2073.6 and 0.576 Ω); tests 1–2 and 1–3 at 42.30144 Ω and 0.45 kW, 2–3 on end 2 at 0.0078336 Ω and 0.6 kW | terminal maps `1,4`, `1,4`, `4,2`; `r_pct` 0.6, 1.2, 1.2; `xsc_pct` 2.04, 2.04, 1.36 |
  | open wye / open delta bank `od` (mirrors `xfmr_open_wye_open_delta.dss`) | two 2000 kVA tanks, 7200 V ends `A`, `B` to 4160 V ends `AB`, `BC`; r 0.5% each; test 1.4904 Ω (5.75% on 25.92 Ω), 20 kW; one `Asset` lists both tanks | `od_t1` maps `1,4` and delta `1,2`; `od_t2` maps `2,4` and delta `2,3`; `xsc_pct` 5.75 |
  | single phase `sp_tank` (mirrors `xfmr_single_phase.dss`) | 25 kVA, 7200/240 V; r 0.6% each; test 0.3 kW; `NoLoadTest` 0.05 kW, `excitingCurrent` 0.538516% | `xsc_pct` 2.04; `%noloadloss = 100 · 50 / 25000 = 0.2`, `%imag = sqrt(0.538516² − 0.2²) = 0.5` |
  | delta/wye `dy_tank` (mirrors `xfmr_delta_wye.dss`) | 300 kVA, 12470 V `D` to 208 V `Y`, ends `ABC`, clock 0 and 1; r 0.5% each; test 3 kW | maps `1,2,3` (delta) and `1,2,3,4` (wye); `xsc_pct` 5.75; `cim_phase_angle_clock` `[0, 1]` |
  | regulator bank `reg` (IEEE 13 pattern) | three 1666 kVA, 2400 V tanks on phases A, B, C, one `Asset` each on a shared `reg_info`; r 0.005% each, x 0.01%; `RatioTapChanger` on end 2 at steps 10, 8, 11 of 0.625% | taps 1.0625, 1.05, 1.06875 on winding 2; `xsc_pct` 0.01 |
  | `PowerTransformerEnd` form `wd` (mirrors `xfmr_wye_delta.dss`) | 500 kVA, 12470 V `Yn` to 480 V `D`; `TransformerCoreAdmittance` on end 1, g 3.2154e-6 S, b 1.2862e-5 S (0.1% and 0.4% of 1/311.0018 Ω) | `%noloadloss` 0.1, `%imag` 0.4 |

## Not vendored

Real GridAPPS-D and CIMHub feeders (IEEE 13, 34, 123, and 8500 in CIM XML,
from [`GRIDAPPSD/Powergrid-Models`](https://github.com/GRIDAPPSD/Powergrid-Models)
and [`GRIDAPPSD/CIMHub`](https://github.com/GRIDAPPSD/CIMHub)) and the EPRI
9500 node feeder ([`epri-dev/GMDM-IOP`](https://github.com/epri-dev/GMDM-IOP))
are the natural corpus. They are not vendored: their license terms need
checking first, and the larger ones exceed the 100 KiB fixture limit.
