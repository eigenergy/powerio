# BMOPF field mapping

What every BMOPF field becomes in PowerIO, in which unit, and which
constraint or objective term it enters. This page is the authority for the
BMOPF converter: a field that reads or writes differently from what is
written here is a defect in one of the two.

Schema versions are named by the [`dsopt-schema`](https://github.com/distribution-system-opt/dsopt-schema)
repository. `0.1.0` is the version the IEEE PES Task Force on Benchmarking
Multiconductor OPF accepts; `0.2.0` is the proposal that adds the element
classes 0.1.0 has no table for. PowerIO reads both and writes `0.2.0` by
default; `BmopfEmitOptions::with_profile(BmopfProfile::Bmopf010)` writes
`0.1.0`.

## Conventions

- Every BMOPF quantity is SI and absolute: volts, amperes, watts, vars,
  volt-amperes, ohms, siemens, metres, radians, hertz. Cost rate is currency
  per kilowatt-hour. PowerIO stores the same units with no scaling, so a
  mapping below states a unit only where the two names differ.
- A matrix element `A_k_j` is row `k`, column `j`, one-based, and becomes row
  `k - 1`, column `j - 1` of the corresponding `ConductorMatrix`. The reader
  mirrors an unstated transpose cell, since these matrices are symmetric; a
  stated cell wins over its mirror.
- A per-terminal array has one entry per name in the element's own terminal
  map, in that order, and keeps that order in PowerIO.
- An absent constraint field is no constraint, and reads as `None`. An absent
  parameter field is zero.
- A field with no typed slot is kept, not dropped: it lands in the element's
  `extras` and the reader reports it under `READ.BMOPF.RETAINED_SOURCE_ONLY`.
  The [retained-only inventory](#fields-with-no-typed-slot) lists every one.

## Document level

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `name` | `PioModule` value name | |
| `meta.$schema` | resolved by `BmopfProfile::from_schema_id` | Absent raises `READ.BMOPF.SCHEMA_ABSENT`; a value naming no version raises `READ.BMOPF.SCHEMA_UNKNOWN`. Both parse, and both versions are accepted. |
| `meta.schema_version` | writer-owned | Stamped on a `0.2.0` write only; `0.1.0` declares no such field. |
| `meta.frequency` | `MulticonductorNetwork::base_frequency`, Hz | Absent defaults to 60 with `READ.BMOPF.VALUE_DEFAULTED`. |
| `meta.*` (the rest) | `MulticonductorNetwork::extras["bmopf_meta"]` | Re-emitted, except the three the writer owns. |
| `terminal_conventions` | `MulticonductorNetwork::extras["bmopf_terminal_conventions"]` | Re-emitted verbatim; authored from the terminal names when the source states none. |
| `extras` | `MulticonductorNetwork::extras["bmopf_extras"]` | Re-emitted verbatim, minus the tables the reader types out of it. |

## bus

`DistBus`, in `MulticonductorNetwork::buses()`.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `terminal_names` | `terminals` | Ordered; fixes every per-terminal order on this bus. |
| `perfectly_grounded_terminals` | `grounded` | |
| `v_min`, `v_max` | `v_min`, `v_max` | The BMOPF array is per phase terminal; PowerIO holds one value, so a genuine per-phase difference raises `READ.BMOPF.VALUE_COLLAPSED` rather than being dropped. |
| `vpn_min`, `vpn_max` | `vpn_min`, `vpn_max` | Per phase terminal, kept as arrays. |
| `vpp_min`, `vpp_max` | `vpp_min`, `vpp_max` | Per ordered phase pair. |
| `vpos_min`, `vpos_max` | `vpos_min`, `vpos_max` | Scalars. |
| `vneg_max`, `vzero_max` | `vneg_max`, `vzero_max` | Magnitude caps; the lower bound is always zero. |
| `vn_max` | `vn_max` | Neutral to ground cap. |
| `longitude`, `latitude` | `location` and `MulticonductorNetwork::geo` | The BMOPFTools coordinate fields, outside both schema versions. Read into the coordinate space; written back only with `BmopfEmitOptions::sideload_coordinates`. |

## line and linecode

`DistLine` and `DistLineCode`.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `line.bus_from`, `line.bus_to` | `bus_from`, `bus_to` | |
| `line.terminal_map_from`, `line.terminal_map_to` | `terminal_map_from`, `terminal_map_to` | Position `i` of the from map fixes matrix index `i`. |
| `line.linecode` | `linecode` | |
| `line.length` | `length`, m | |
| `line.R_series_i_j`, `line.X_series_i_j` | a synthesized `DistLineCode` named after the line, ohm per metre | The inline branch states absolute ohms, so the reader divides by `length` to store per metre and keeps the line's own length. `READ.BMOPF.VALUE_INFERRED` names the synthesis. |
| `line.G_from_i_j`, `line.B_from_i_j`, `line.G_to_i_j`, `line.B_to_i_j` | the same synthesized line code's `g_from`, `b_from`, `g_to`, `b_to` | |
| `line.i_max`, `line.s_max` | `i_max`, `s_max` | Per conductor; override the line code's. |
| `linecode.R_series_i_j`, `linecode.X_series_i_j` | `r_series`, `x_series`, ohm per metre | |
| `linecode.G_from_i_j`, `linecode.B_from_i_j`, `linecode.G_to_i_j`, `linecode.B_to_i_j` | `g_from`, `b_from`, `g_to`, `b_to`, siemens per metre | Half the total shunt at each end. |
| `linecode.i_max`, `linecode.s_max` | `i_max`, `s_max` | Per conductor, applied at both ends. |
| `linecode.source` | `source` | |
| `linecode.line_geometry`, `linecode.derivation` | `extras` | 0.2.0 fields; retained, not typed. |

A line carries exactly one impedance source. The `oneOf` of both versions
requires either a `linecode` with a `length`, or inline `R_series_1_1` and
`X_series_1_1` with no `linecode`.

## switch

`DistSwitch`.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus_from`, `bus_to`, `terminal_map_from`, `terminal_map_to` | the same names | |
| `open_switch` | `open` | |
| `i_max` | `i_max` | Per conductor. A switch has no shunt, so both ends carry the same magnitude and one array bounds both. |

## load

`DistLoad`. Arrays are per sub-load: one phase-to-neutral branch for `WYE`,
the terminal pair for `SINGLE_PHASE`, one line-to-line branch for `DELTA`.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus`, `terminal_map`, `configuration` | the same names | `configuration` reads case-insensitively; an unrecognized value reads as `WYE` with `READ.BMOPF.VALUE_UNSUPPORTED`. |
| `p_nom`, `q_nom` | `p_nom`, `q_nom`, W and var | |
| `model` | the `DistLoadVoltageModel` variant | `CONSTANT_POWER`, `CONSTANT_CURRENT`, `CONSTANT_IMPEDANCE`, `ZIP`, `EXPONENTIAL`. |
| `v_nom` | the variant's `v_nom` | |
| `alpha_z`, `alpha_i`, `alpha_p`, `beta_z`, `beta_i`, `beta_p` | the `Zip` variant's fields | The three active fractions sum to one, and so do the three reactive. |
| `gamma_p`, `gamma_q` | the `Exponential` variant's fields | |

## generator

`DistGenerator`. Arrays are per phase conductor; `WYE` is the only
configuration the specification supports.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus`, `terminal_map`, `configuration` | the same names | |
| `p_min`, `p_max`, `q_min`, `q_max` | the same names, W and var | When a bound pair is equal the dispatch is pinned, and the reader states the same values as `p_nom` and `q_nom` so a power flow target has a setpoint. |
| `s_max` | `s_max`, VA | Bounds the sum of squares of that phase's active and reactive power. |
| `i_max` | `i_max`, A | Per phase, with an optional trailing entry bounding the neutral return current. |
| `cost` | `cost`, currency per kWh | Kept exactly as stated: one entry per phase. A bare scalar reads as a one-entry statement. |

## voltage_source

`VoltageSource`. Both versions permit exactly one.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus`, `terminal_map` | the same names | |
| `v_magnitude` | `v_magnitude`, V | Per terminal, phase to ground; a grounded terminal states zero. |
| `v_angle` | `v_angle`, rad | Per terminal. |
| `cost` | `extras["cost"]` | A 0.2.0 field; retained and re-emitted, not typed. |
| `p_min`, `p_max`, `q_min`, `q_max` | `extras` | The same standing as `cost`. |

## shunt and capacitor

`DistShunt` carries raw admittance, including grounding impedance;
`DistCapacitor` carries a nameplate-rated bank.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `shunt.bus`, `shunt.terminal_map` | the same names | |
| `shunt.G_i_j`, `shunt.B_i_j` | `g`, `b`, total siemens in conductor order | |
| `capacitor.bus`, `capacitor.terminal_map`, `capacitor.configuration` | the same names | |
| `capacitor.q_rated` | `q_rated`, var | The whole bank, not one element. |
| `capacitor.v_nom` | `v_nom`, V | Line to line for the three-phase configurations; across the element terminals for `SINGLE_PHASE`. |

## transformer

`DistTransformer` with `windings: Vec<DistWinding>`. The BMOPF subtype rides
in `DistTransformer::extras["bmopf_subtype"]`, because the winding list alone
does not pin down every subtype: a centre tap unit reads as two secondary
windings.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus_from`, `bus_to` | `windings[0].bus`, `windings[1].bus` | |
| `terminal_map_from`, `terminal_map_to` | the corresponding `terminal_map` | `center_tap` expands the three to-side terminals into two windings. |
| `v_nom_from`, `v_nom_to` | `windings[k].v_ref`, V | For `center_tap`, `v_nom_to` is the per leg voltage. |
| `s_rating` | `windings[k].s_rating`, VA | |
| `r_series_from`, `r_series_to` | `windings[k].r_pct`, percent of that winding's own base | The base is `n_phases * v_ref^2 / s_rating`. |
| `x_series_from`, `x_series_to` | `xsc_pct`, percent | The short-circuit test measures the series sum referred to one side, which is what this field is. |
| `r_series`, `x_series` (three-phase legacy) | `windings[0].r_pct` and `xsc_pct` | The lumped wye-side spelling; the delta winding is lossless in this model. |
| `tap_ratio` (0.2.0), `tap` (retained under 0.1.0) | `windings[0].tap / windings[1].tap` | The multiplier on the nameplate turns ratio. |
| `tap_ratio_min`, `tap_ratio_max` | `extras` | Bounds have no typed winding slot. |
| `r_neutral_from`, `x_neutral_from` | `windings[0].r_neutral`, `x_neutral`, ohm | The winding's own neutral to earth branch. |
| `r_neutral_to`, `x_neutral_to` | `windings[1].r_neutral`, `x_neutral` | |
| `g_no_load`, `b_no_load` | `extras` | The magnetising branch has no typed slot. |
| `i_max_from`, `i_max_to` | `extras` | Per winding conductor of that side, in that side's own amperes. |
| `n_winding.windings[]` | one `DistWinding` each | `bus`, `terminal_map`, `v_nom`, `configuration`, `r_winding`, `delta_roll`, `i_max`. |
| `n_winding.x_sc` | `xsc_pct`, in `[xhl, xht, xlt]` order | Keyed `i_j` with `i < j` in BMOPF, all referred to winding 1. |
| `single_phase_autotransformer`, `open_delta_regulator` | windings plus `extras["bmopf_subtype"]` | The regulator ratio, its bounds, the ANSI type and the open delta connection ride in `extras`. |

Under schema `0.1.0` the nine fields with no subtype slot (`tap`, `tap_min`,
`tap_max`, the four winding neutral fields, and the two no-load fields) are
written to `extras.transformer.<subtype>.<name>` and folded back on read, each
move reported under `EMIT.BMOPF.RETAINED_SOURCE_ONLY`. Under `0.2.0` the
subtypes declare all nine and nothing moves; only the three tap names change,
to `tap_ratio`, `tap_ratio_min`, `tap_ratio_max`.

## ibr and control_profile

`DistIbr` and `DistControlProfile`. Both are typed under either version;
`0.1.0` has no top-level table for them, so they are written under `extras`
and read from either place, with the top-level copy winning.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus`, `terminal_map` | the same names | |
| `topology` | `topology`: `SINGLE_PHASE`, `THREE_LEG`, `FOUR_LEG` | |
| `prime_mover` | `prime_mover`: `PV`, `BATTERY`, `GENERIC`, `STATCOM`, `DSTATCOM` | |
| `s_max` | `s_max`, VA per phase | |
| `i_max` | `i_max`, A per conductor | |
| `p_avail` | `p_avail`, W | |
| `p_min`, `p_max`, `q_min`, `q_max` | the same names, per phase | |
| `control_profile` | `control_profile`, an id | |
| `voltage_aggregation` | `voltage_aggregation`: `PER_PHASE`, `AVERAGE` | |
| `cost` | `extras["cost"]` | Retained and re-emitted, not typed. |
| `dc_bus`, `dc_terminal_map`, `dc_control`, `dc_v_set`, `dc_p_ref`, `dc_droop`, `dc_deadband`, `dc_link_coupled`, `p_dc_min`, `p_dc_max` | `extras` | The DC coupling fields; retained, not typed. |
| `r_filter`, `x_filter`, `b_filter_shunt`, `grid_forming`, `v_ref_internal` | `extras` | Retained, not typed. |
| `control_profile.power_factor.pf` | `PowerFactorControl` | |
| `control_profile.volt_var.*` | `VoltVarControl`: `voltage_reference`, `breakpoints`, `q_limits`, `q_unit`, `q_ref`, and the two active-power thresholds | |
| `control_profile.volt_watt.*` | `VoltWattControl`: `voltage_reference`, `breakpoints`, `p_limits`, `p_unit`, `p_ref` | |

## Fields with no typed slot

These classes read into `MulticonductorNetwork::untyped()` by class and name,
with their properties kept as text, and are written back to the table the
target version declares: the top level under `0.2.0`, `extras` under `0.1.0`.
Reading each reports `READ.BMOPF.RETAINED_SOURCE_ONLY`.

`dc_bus`, `dc_branch`, `dc_grounding`, `dc_load`, `dc_source`, `time_series`,
`wire_data`, `line_geometry`, and a per-element `time_series` reference map.

They travel and re-emit unchanged. Nothing reads their values into a
calculation, so a case whose behaviour depends on them is not represented by
the instance PowerIO builds from it. The reader says so per class rather than
leaving it to be discovered.

## The calculation

`powerio::to_mc_ac_opf_instance` builds a `McAcOpfInstance` through
`McAcOpfInstance::from_network`. The network is shared, not copied: the
instance selects which of the network's stated limits are active constraints
and which objective terms are summed, and reads the numbers from the network.

**Objective.** One term, `ObjectiveTerm::ActivePowerDispatchCost`. It is the
specification's default objective: the sum over every dispatchable element and
every phase of that phase's cost rate against the active power the element
injects into the network. A positive cost minimises the element's injection
and a negative cost maximises it, uniformly for a generator, the voltage
source, and an IBR. The rate comes from the element's own per phase `cost`
array, so a generator reads it from `DistGenerator::cost` and a voltage source
or IBR from its `extras["cost"]`. `ObjectiveTerm::NetworkGeneratorCost` is the
balanced-network term and is not part of a BMOPF instance.

**Constraints.** `MulticonductorActiveConstraints` selects three families, each
`ConstraintSelection::All` by default, which matches the specification's rule
that constraints are active for the elements present.

| Family | The BMOPF fields it activates |
| --- | --- |
| `terminal_voltage_bounds` | `bus.v_min`, `v_max`, `vpn_min`, `vpn_max`, `vpp_min`, `vpp_max`, `vpos_min`, `vpos_max`, `vneg_max`, `vzero_max`, `vn_max` |
| `conductor_limits` | `line.i_max`, `line.s_max`, `linecode.i_max`, `linecode.s_max`, `switch.i_max`, `transformer.i_max_from`, `transformer.i_max_to` |
| `generator_capability` | `generator.p_min`, `p_max`, `q_min`, `q_max`, `s_max`, `i_max`, and the same bounds on an IBR |

A family selects by stable element identity, so a study relaxing one line's
thermal limit names that line rather than restating the bound.
`ConstraintSelection::None` relaxes a family; `Only` names exactly the
elements whose limits stay active.

Equipment behaviour is not a selectable family, because it is not a limit: a
closed switch equates its two ends conductor by conductor, an open switch
carries no current, the ideal winding pair relates its two coil voltages by
the turns ratio and balances ampere-turns, and the voltage source fixes its
terminal voltage with a free current. Those hold for every instance built from
a network that states them.

## The solution

Solved values stay in `McAcOpfSolution` and never travel back into the
network, so the network keeps stating the case and the solution states one
result of it.

| `McAcOpfSolution` | Unit and order |
| --- | --- |
| `terminal_voltage_magnitude` | V, resolved terminal order |
| `terminal_voltage_angle` | rad, resolved terminal order |
| `terminal_current_magnitude` | A, resolved terminal order, when the solver reports it |
| `terminal_active_power` | W, resolved terminal order, when the solver reports it |
| `source_active_injection` | W, per source terminal |
| `generator_active_power` | W, generator table order with each generator's terminal map order |
| `objective` | the optimised objective value |
| `termination`, `residuals`, `producer` | how the solve ended, its residuals, and what produced it |

## Checked against what

The two published example networks, vendored at
`tests/data/dist/bmopf/`, are the reference data. `powerio-dist/tests/bmopf.rs`
checks that each parses, that writing the result validates against the schema
of the version written, that a second write is identical to the first, and
that parsing the written document reproduces the model. The equations above
are checked against the specification pages of
[`math-and-data-model-specifications`](https://github.com/distribution-system-opt/math-and-data-model-specifications),
not inferred from the data.

BMOPFTools.jl is the Task Force toolchain that generated `example_ieee13.json`,
which names it in `meta.case_study_generator`. It is not published in the
`distribution-system-opt` organization, so the comparison closed issue #414
asked for, against its admittance stamps, constraint activation, objective
terms, conductor order, and regulator behaviour, is not made here. What is
checked against it is the data it writes: the one-triangle matrix spelling its
writer uses reads back with the mirrored cells filled, and the regulator
subtypes it extends the schema with read and write as
`transformer.single_phase_autotransformer` and
`transformer.open_delta_regulator`. Comparing the numbers a solve produces
needs the toolchain.
