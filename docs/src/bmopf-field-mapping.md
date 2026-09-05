# BMOPF field mapping

This page says what each BMOPF field becomes in PowerIO, in which unit, and
which constraint or objective term it enters. It is the authority for the
BMOPF converter, so if you find a field that reads or writes differently from
what is written here, one of the two has a defect.

Schema versions come from the [`dsopt-schema`](https://github.com/distribution-system-opt/dsopt-schema)
repository. `0.1.0` is the version the IEEE PES Task Force on Benchmarking
Multiconductor OPF accepts, and `0.2.0` is the proposal that adds the element
classes 0.1.0 has no table for. PowerIO reads both and writes `0.2.0` by
default; to write `0.1.0`, pass
`BmopfEmitOptions::with_profile(BmopfProfile::Bmopf010)`.

## Conventions

Every BMOPF quantity is SI and absolute: volts, amperes, watts, vars,
volt-amperes, ohms, siemens, metres, radians, hertz, and a cost rate in
currency per kilowatt-hour. PowerIO stores the same units with no scaling, so
a mapping below gives a unit only where the two names differ.

A matrix element `A_k_j` is row `k`, column `j`, counting from one, and lands
in row `k - 1`, column `j - 1` of the corresponding `ConductorMatrix`. These
matrices are symmetric, so the reader fills an unstated transpose cell from
its mirror; a stated cell wins over its mirror. A per terminal array has one
entry per name in the element's own terminal map, in that order, and PowerIO
keeps that order.

An absent constraint field means no constraint and reads as `None`; an absent
parameter field is zero. A field with no typed slot lands in the element's
`extras`, and the reader reports it under `READ.BMOPF.RETAINED_SOURCE_ONLY`.
[Fields with no typed slot](#fields-with-no-typed-slot) lists every one.

## Document level

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `name` | `PioModule` value name | |
| `meta.$schema` | resolved by `BmopfProfile::from_schema_id` | Absent raises `READ.BMOPF.SCHEMA_ABSENT`; a value naming no version raises `READ.BMOPF.SCHEMA_UNKNOWN`. Both parse, and both versions are accepted. |
| `meta.schema_version` | explicit profile identity | The reader checks agreement with `meta.$schema`. Fresh proposal output pins a retrieval URL and records proposal status and schema digest in provenance. |
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
| `v_min`, `v_max` | `v_min_phase`, `v_max_phase` and scalar `v_min`, `v_max` | Unequal bounds remain ordered phase vectors through IR and bindings. A scalar edit explicitly overrides the vector; balanced lowering rejects unequal phase bounds. |
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

A line has exactly one impedance source, and the `oneOf` in both schema
versions enforces it: either a `linecode` with a `length`, or inline
`R_series_1_1` and `X_series_1_1` with no `linecode`.

## switch

`DistSwitch`.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus_from`, `bus_to`, `terminal_map_from`, `terminal_map_to` | the same names | |
| `open_switch` | `open` | |
| `i_max` | `i_max` | Per conductor. A switch has no shunt, so both ends carry the same magnitude and one array bounds both. |

## load

`DistLoad`. Each array has one entry per branch of the load: a phase to
neutral branch for `WYE`, the terminal pair for `SINGLE_PHASE`, or a line to
line branch for `DELTA`.

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

`DistShunt` is raw admittance, grounding impedance included; `DistCapacitor`
is a bank with a nameplate rating.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `shunt.bus`, `shunt.terminal_map` | the same names | |
| `shunt.G_i_j`, `shunt.B_i_j` | `g`, `b`, total siemens in conductor order | |
| `capacitor.bus`, `capacitor.terminal_map`, `capacitor.configuration` | the same names | |
| `capacitor.q_rated` | `q_rated`, var | The whole bank, not one element. |
| `capacitor.v_nom` | `v_nom`, V | Line to line for the three-phase configurations; across the element terminals for `SINGLE_PHASE`. |

## transformer

`DistTransformer` with `windings: Vec<DistWinding>`. The BMOPF subtype is
kept in `DistTransformer::extras["bmopf_subtype"]`, because the winding list
alone does not pin down every subtype; a centre tap unit, for example, reads
as two secondary windings.

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

Under schema `0.1.0`, the nine fields that have no subtype slot (`tap`,
`tap_min`, `tap_max`, the four winding neutral fields, and the two no-load
fields) are written to `extras.transformer.<subtype>.<name>` and folded back
on read, with each move reported under `EMIT.BMOPF.RETAINED_SOURCE_ONLY`.
Under `0.2.0` the subtypes declare all nine, so nothing moves; only the three
tap names change, to `tap_ratio`, `tap_ratio_min`, and `tap_ratio_max`.

## ibr and control_profile

`DistIbr` and `DistControlProfile`. Both are typed under either version.
Because `0.1.0` has no top-level table for them, that version writes them
under `extras`; on read they come from either place, and the top-level copy
wins.

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

The classes below read into `MulticonductorNetwork::untyped()` by class and
name, with their properties kept as text, and are written back to the table
the target version declares: the top level under `0.2.0`, `extras` under
`0.1.0`. Reading each one reports `READ.BMOPF.RETAINED_SOURCE_ONLY`.

`dc_bus`, `dc_branch`, `dc_grounding`, `dc_load`, `dc_source`, `time_series`,
`wire_data`, `line_geometry`, and a per-element `time_series` reference map.

They pass through and re-emit unchanged. Nothing reads their values into a
calculation, so if a case's behaviour depends on them, the instance PowerIO
builds from it does not represent that case. The reader says so per class
rather than leaving you to discover it.

## The calculation

`powerio::to_mc_ac_opf_instance` builds a `McAcOpfInstance` through
`McAcOpfInstance::from_network`. The instance shares the network rather than
copying it: it selects which of the network's stated limits are active
constraints and which objective terms are summed, and reads the numbers from
the network.

**Objective.** There is one term, `ObjectiveTerm::ActivePowerDispatchCost`,
which is the specification's default objective: the sum over every
dispatchable element and every phase of that phase's cost rate against the
active power the element injects into the network. A positive cost minimises
the element's injection and a negative cost maximises it, and that holds the
same way for a generator, the voltage source, and an IBR. The rate comes from
the element's own per phase `cost` array: a generator reads it from
`DistGenerator::cost`, and a voltage source or IBR from its `extras["cost"]`.
`ObjectiveTerm::NetworkGeneratorCost` is the term for a balanced network and
is not part of a BMOPF instance.

**Constraints.** `MulticonductorActiveConstraints` selects three families, each
`ConstraintSelection::All` by default, which matches the specification's rule
that constraints are active for the elements present.

| Family | The BMOPF fields it activates |
| --- | --- |
| `terminal_voltage_bounds` | `bus.v_min`, `v_max`, `vpn_min`, `vpn_max`, `vpp_min`, `vpp_max`, `vpos_min`, `vpos_max`, `vneg_max`, `vzero_max`, `vn_max` |
| `conductor_limits` | `line.i_max`, `line.s_max`, `linecode.i_max`, `linecode.s_max`, `switch.i_max`, `transformer.i_max_from`, `transformer.i_max_to` |
| `generator_capability` | `generator.p_min`, `p_max`, `q_min`, `q_max`, `s_max`, `i_max`, and the same bounds on an IBR |

A family selects by stable element identity, so a study that relaxes one
line's thermal limit refers to that line instead of restating the bound.
`ConstraintSelection::None` relaxes a whole family, and `Only` lists the
elements whose limits stay active.

Equipment behaviour has no selection family, because there is no limit to
relax: a closed switch equates its two ends conductor by conductor, an open
switch carries no current, the ideal winding pair relates its two coil
voltages by the turns ratio and balances ampere-turns, and the voltage source
fixes its terminal voltage with a free current. Those hold in every instance
built from a network that includes them.

## The solution

Solved values stay in `McAcOpfSolution` and are not written back into the
network, so the network still describes the case and the solution describes
one result of it.

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

The reference data is the two published example networks, vendored at
`tests/data/dist/bmopf/`. `powerio-dist/tests/bmopf.rs` checks that each one
parses, that writing the result validates against the schema of the version
written, that a second write is identical to the first, and that parsing the
written document reproduces the model. The equations above were checked
against the specification pages of
[`math-and-data-model-specifications`](https://github.com/distribution-system-opt/math-and-data-model-specifications)
rather than inferred from the data.

BMOPFTools.jl is the Task Force toolchain that generated `example_ieee13.json`,
which lists it in `meta.case_study_generator`. The toolchain is not published
in the `distribution-system-opt` organization, so the comparison that closed
issue #414 asked for (against its admittance stamps, constraint activation,
objective terms, conductor order, and regulator behaviour) is not made here.
What is checked is the data it writes: the matrix spelling its writer uses,
with only one triangle filled in, reads back with the mirrored cells filled,
and the regulator subtypes it adds to the schema read and write as
`transformer.single_phase_autotransformer` and
`transformer.open_delta_regulator`. Comparing the numbers a solve produces
would need the toolchain itself.

## Explicit terminal-coil no-load admittance

`transformer.<subtype>.<id>.no_load_shunt` holds `{winding, g, b}` in transformer
extras and generation-2 IR. The one-based winding index fixes its physical
location, and `g + j b` is siemens per coil at the terminal voltage. It cannot
coexist with the existing from-side `g_no_load` and `b_no_load` fields.

OpenDSS and PMD exciting-branch percentages map to winding 2 with a negative
magnetizing susceptance. Conversion uses the actual tapped WYE phase-to-neutral
or DELTA phase-to-phase coil voltage and divides total transformer VA by the
phase count. A winding-2 shunt converts back to those percentages; other
locations report that this target parameterization cannot represent them.
Legacy BMOPF output preserves the object under `extras.transformer` and reports
the relocation. Nonzero core shunts reject the limited PowerIO passive
transformer matrix profile before execution.

The independent check `evals/validation/validate_bmopf_core_shunts.py` compares
six transformer topologies with OpenDSS `Yprim` through an intermediate PowerIO
IR document. BMOPFTools' 0.11 adapter uses an equivalent bus-shunt matrix and
retains the original coil object in provenance. Successful parsing alone does
not establish support for a transformer calculation.
