# BMOPF field mapping

This page says what each BMOPF field becomes in PowerIO, in which unit, and
which constraint or objective term it enters. It is the authority for the
BMOPF converter, so if you find a field that reads or writes differently from
what is written here, one of the two has a defect.

The schema comes from the Task Force repository
[`dsopt-schema`](https://github.com/distribution-system-opt/dsopt-schema).
There is one BMOPF: PowerIO reads and writes the schema proposed in
[dsopt-schema#2](https://github.com/distribution-system-opt/dsopt-schema/pull/2)
at commit `6796394`, the Task Force's schema plus the element classes the
IEEE PES Task Force on Benchmarking Multiconductor OPF draft carried. The pin
moves to the release tag the schema's `$id` names once the Task Force ratifies
it; the `$id` is already the URL of the schema's bytes at that tag.

A document from an older BMOPF draft (including the `bmopf-report` drafts and
the early draft PowerIO 0.11 wrote) is read as BMOPF: the reader also accepts
the older spellings and layouts listed on this page, and reports
`READ.BMOPF.SCHEMA_LEGACY` once. Writing it produces a document in the
current schema; the source bytes are echoed only for a document that states a
current `meta.schema_version` and no `meta.$schema`.

The schema renamed some tokens. The reader takes the earlier spellings, and a
current document that uses them gets the same single
`READ.BMOPF.SCHEMA_LEGACY`; a write states the current names:

| Earlier spelling | Current | Model value |
| --- | --- | --- |
| `meta.$schema` | `meta.schema_version` only | none; the `$id` goes in provenance |
| bus `v_min`, `v_max`, `vn_max` | `vpg_min`, `vpg_max`, `vng_max` | `v_min_phase`, `v_max_phase`, `vn_max` |
| regulator `tap_ratio`, `tap_ratio_min`, `tap_ratio_max` (and `time_series` keys) | `voltage_ratio`, `voltage_ratio_min`, `voltage_ratio_max` | `windings[1].tap / windings[0].tap`, `extras["tap_ratio_min"]`, `extras["tap_ratio_max"]` |
| `voltage_source.cost`, `ibr.cost` | `energy_cost_rate` | `VoltageSource::energy_cost_rate`, `extras["energy_cost_rate"]` |
| `prime_mover` `DSTATCOM` | `STATCOM` | `IbrPrimeMover::Statcom` (a model `Dstatcom` writes as `STATCOM`, reported) |
| `q_unit`, `p_unit` `VA_FRACTION` | `PU` | `VaFraction` |
| `dc_control` `droop` | `DROOP` | the string in `extras` |
| `wire_data.kind` `overhead`, `cn_cable`, `ts_cable`; `line_geometry.earth_model` `modified_carson`, `full_carson`, `deri` | UPPER_SNAKE | retained source data |

The writer never states a value the schema rejects. A field outside its
constraint (a tap or ratio that is not strictly positive, a control law of the
wrong shape, a bus bound the bus cannot carry, a limit array of the wrong
length) is dropped with `EMIT.BMOPF.FIELD_DROPPED`; a record whose required
rating or nominal voltage is not strictly positive is dropped whole with
`EMIT.BMOPF.RECORD_DROPPED`. A network with no voltage source still writes an
empty `voltage_source` table, which the schema rejects; `EMIT.BMOPF.SOURCE_COUNT`
reports it.

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
| `meta.schema_version` | identifies the document | The document's one version identifier, which the schema requires. Written as the release the schema's `$id` names; any patch release of that line reads as current. An older draft's release raises `READ.BMOPF.SCHEMA_LEGACY`, and another value `READ.BMOPF.SCHEMA_UNKNOWN`. Output records the schema's `$id` (`BMOPF_SCHEMA_ID`), the proposal commit, the schema digest, and the retrieval permalink (`bmopf_proposal_url()`) under `meta.provenance.powerio_bmopf`. |
| `meta.$schema` | identifies a document from an older draft | Removed in the current schema and never written. A document without `meta.schema_version` is identified by it, as the older drafts defined: their identifiers, the `bmopf-report` draft locations, the early draft permalinks, and the PowerIO archive URLs that 0.11 stamped raise `READ.BMOPF.SCHEMA_LEGACY`; the current schema's location reads as current with the key counted as an earlier spelling. A current document carrying it reads too, counted the same way. A `$schema` and a release number naming different lines raise `READ.BMOPF.SCHEMA_MISMATCH`. Neither stated raises `READ.BMOPF.SCHEMA_ABSENT`. Every one parses, read as BMOPF. |
| `meta.crs` | `MulticonductorNetwork::geo` coordinate space | The authority code of every `geo` field's system; absent means `OGC:CRS84`, WGS84 longitude then latitude, and coordinates are always x then y, so `EPSG:4326` reads the same way. Written for a projected or non-WGS84 system, omitted for WGS84. A space the schema cannot name (unknown, diagram, a code-less projection) or coordinates not from the source also travel as `extras.powerio_geo`, which the reader prefers. |
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
| `vpg_min`, `vpg_max`; read also as `v_min`, `v_max` (the first draft) | `v_min_phase`, `v_max_phase` and scalar `v_min`, `v_max` | Phase to ground, one entry per phase terminal. Written as `vpg_*`. Both spellings read, the current one first; a current document that uses `v_*` raises one `READ.BMOPF.SCHEMA_LEGACY` naming the spellings, since that schema rejects them, and both spellings disagreeing raises `READ.BMOPF.SEMANTIC_INVALID`. Unequal bounds remain ordered phase vectors through IR and bindings. A scalar edit explicitly overrides the vector; balanced lowering rejects unequal phase bounds. |
| `vpn_min`, `vpn_max` | `vpn_min`, `vpn_max` | Per phase terminal, kept as arrays. |
| `vpp_min`, `vpp_max` | `vpp_min`, `vpp_max` | Per ordered phase pair. |
| `vpos_min`, `vpos_max` | `vpos_min`, `vpos_max` | Scalars. |
| `vneg_max`, `vzero_max` | `vneg_max`, `vzero_max` | Magnitude caps; the lower bound is always zero. |
| `vng_max`; read also as `vn_max` (older drafts) | `vn_max` | Neutral to ground cap. Written as `vng_max`. A current document that uses `vn_max` raises the same single `READ.BMOPF.SCHEMA_LEGACY` as `v_*`, and both spellings disagreeing raises `READ.BMOPF.SEMANTIC_INVALID`. |
| `geo` | `location` and `MulticonductorNetwork::geo` | A GeoJSON Point in the system `meta.crs` names. Written for every bus with a finite location. The reader also accepts the `extras.geojson` FeatureCollection that PowerIO 0.11 wrote, when no element carries a `geo` field; it is never written. |
| `longitude`, `latitude` | `location` and `MulticonductorNetwork::geo` | The pre-schema BMOPFTools coordinate fields, outside every schema version. Read into the coordinate space as WGS84; never written back. |

## line and linecode

`DistLine` and `DistLineCode`.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `line.bus_from`, `line.bus_to` | `bus_from`, `bus_to` | |
| `line.terminal_map_from`, `line.terminal_map_to` | `terminal_map_from`, `terminal_map_to` | Position `i` of the from map fixes matrix index `i`. |
| `line.linecode` | `linecode` | |
| `line.length` | `length`, m | |
| `line.R_series_i_j`, `line.X_series_i_j` | a synthesized `DistLineCode` named after the line, ohm per metre at unit length | The inline branch states the absolute ohms of the whole line, and its `length` is descriptive. The reader keeps those ohms unscaled in the synthesized line code and sets the line's length to 1 m, so length times the per metre value is the stated impedance; a stated `length` other than 1 raises `READ.BMOPF.VALUE_DEFAULTED`. `READ.BMOPF.VALUE_INFERRED` names the synthesis. A line that names a `linecode` takes no inline key. |
| `line.G_from_i_j`, `line.B_from_i_j`, `line.G_to_i_j`, `line.B_to_i_j` | the same synthesized line code's `g_from`, `b_from`, `g_to`, `b_to` | |
| `line.i_max`, `line.s_max` | `i_max`, `s_max` | One entry per conductor of `terminal_map_from`; override the line code's. An older draft's line text said `s_max` is per phase: a document stating one entry per phase on a map with a neutral reads per conductor with the neutral unbounded (`READ.BMOPF.VALUE_INFERRED`). A write states `s_max` only when it has one finite entry per conductor, and otherwise drops it with `EMIT.BMOPF.FIELD_DROPPED` rather than inventing a limit. |
| `line.geo` | `route` | A GeoJSON LineString from `bus_from` through any bends to `bus_to`, in the system `meta.crs` names; descriptive, `length` stays the electrical length. Written for every line with a route of two or more finite vertices. |
| `linecode.R_series_i_j`, `linecode.X_series_i_j` | `r_series`, `x_series`, ohm per metre | |
| `linecode.G_from_i_j`, `linecode.B_from_i_j`, `linecode.G_to_i_j`, `linecode.B_to_i_j` | `g_from`, `b_from`, `g_to`, `b_to`, siemens per metre | Half the total shunt at each end. |
| `linecode.i_max`, `linecode.s_max` | `i_max`, `s_max` | Per conductor, in matrix row order, applied at both ends. A write drops an `s_max` of another length. |
| `linecode.source` | `source` | |
| `linecode.line_geometry`, `linecode.derivation` | `extras` | Schema fields; retained on read, not typed, and not written (`EMIT.BMOPF.FIELD_DROPPED`). |

A line has exactly one impedance source, and the `oneOf` in both schema
versions enforces it: either a `linecode` with a `length`, or inline
`R_series_1_1` and `X_series_1_1` with no `linecode`.

## Element geometry

Every located element other than a bus or line may carry a `geo`: a GeoJSON
Point on `voltage_source`, `load`, `generator`, `ibr`, `shunt` and
`capacitor`, and a Point or LineString on `switch` and every transformer
subtype. The DC classes take one too and pass through unchanged with the rest
of their object.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `<element>.geo` Point | a `DistComponentLocation` for that element (`MulticonductorNetwork::component_locations`) | Longitude and latitude in degrees, with `meta.crs` as the datum (`EPSG:4326` when absent). |
| `switch.geo`, `transformer.<subtype>.<name>.geo` LineString | the same, at the midpoint of the LineString's two end positions | Reported once with `READ.BMOPF.VALUE_COLLAPSED`; a write states that Point. |
| `bbox` on any `geo` | not read | Never written. |

A component location is geographic, so a `geo` in a projected document reads
nothing and raises `READ.BMOPF.FIELD_DROPPED`. The writer states a component
location as its element's `geo` Point when the document's geometry is WGS84
(no `meta.crs`) and the point states both axes and a WGS84 datum or none. A
point stating no datum, or an origin other than the source data, reads back
as WGS84 source coordinates, which `EMIT.BMOPF.VALUE_SUBSTITUTED` reports.
The rest, including component points on buses and lines (whose `geo` carries
the bus location and the line route), raise
`EMIT.DIST.COMPONENT_COORDINATES_DROPPED`.

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
| `energy_cost_rate`, or the deprecated `cost` | `cost`, currency per kWh | Kept exactly as stated: one entry per phase. A bare scalar reads as a one-entry statement. Written as `energy_cost_rate`. |

## voltage_source

`VoltageSource`. Both versions permit exactly one.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus`, `terminal_map` | the same names | |
| `v_magnitude` | `v_magnitude`, V | Per terminal, phase to ground; a grounded terminal states zero. |
| `v_angle` | `v_angle`, rad | Per terminal. |
| `energy_cost_rate` | `energy_cost_rate`, currency per kWh | One entry per phase conductor; neutral terminals have no entry. The schema removed `cost` here; a document that still states it reads it as `energy_cost_rate` (with one `READ.BMOPF.SCHEMA_LEGACY` when it states the current schema), and a write never states `cost`. |
| `p_min`, `p_max`, `q_min`, `q_max` | `extras` | Retained and re-emitted, not typed. |

## shunt and capacitor

`DistShunt` is raw admittance, grounding impedance included. Scalar capacitor
ratings use the bank model `DistCapacitor`; arrays of per-coil ratings are
lowered into an exact `DistShunt` terminal matrix.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `shunt.bus`, `shunt.terminal_map` | the same names | |
| `shunt.G_i_j`, `shunt.B_i_j` | `g`, `b`, total siemens in conductor order | |
| `capacitor.bus`, `capacitor.terminal_map`, `capacitor.configuration` | the same names for a scalar bank; bus, terminal order and matrix incidence for an array | |
| `capacitor.q_rated` | Scalar: `q_rated`, var; array: terminal shunt `b`, siemens | A scalar rates the whole bank. Array entries rate individual WYE coils or DELTA phase pairs, in var. |
| `capacitor.v_nom` | Scalar: `v_nom`, V; array: used in `q / v_nom²` | Scalar: line-to-line for three-phase banks, across the pair for `SINGLE_PHASE`. Array: phase-to-neutral for WYE, phase-to-phase for DELTA, and across the stated pair for `SINGLE_PHASE`. |

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
| `tap_ratio`; read also as `tap` | `windings[0].tap / windings[1].tap` | The multiplier on the nameplate turns ratio. |
| `tap_ratio_min`, `tap_ratio_max` | `extras` | Bounds have no typed winding slot. |
| `r_neutral_from`, `x_neutral_from` | `windings[0].r_neutral`, `x_neutral`, ohm | The winding's own neutral to earth branch. |
| `r_neutral_to`, `x_neutral_to` | `windings[1].r_neutral`, `x_neutral` | |
| `g_no_load`, `b_no_load` | `extras` | The magnetising branch has no typed slot. |
| `i_max_from`, `i_max_to` | `extras` | Per winding conductor of that side, in that side's own amperes. |
| `n_winding.windings[]` | one `DistWinding` each | `bus`, `terminal_map`, `v_nom`, `configuration` (`WYE` or `DELTA`), `r_winding`, `delta_roll`, `i_max`, `r_neutral`, `x_neutral`. Both neutral fields are nonnegative; a write drops a negative or nonfinite one with `EMIT.BMOPF.TRANSFORMER_NEUTRAL_DROPPED`, as the two-winding subtypes do. |
| `n_winding.x_sc` | `xsc_pct`, ordered `12, 13, ..., 1n, 23, ..., (n-1)n` | Keyed `i_j` with `i < j` in BMOPF, all referred to winding 1. |
| `single_phase_autotransformer`, `open_delta_regulator` | windings plus `extras["bmopf_subtype"]` | `voltage_ratio` (read also as the earlier `tap_ratio`) is the regulated over source voltage ratio, `V_to/V_from`, for both ANSI types: `windings[1].tap / windings[0].tap`. Its bounds `voltage_ratio_min`/`voltage_ratio_max`, `regulator_type` and the open delta connection ride in `extras`; `regulator_type` records the A/B hardware and does not change the ratio. |

The two-winding subtypes declare the tap ratio and its bounds, the four
winding neutral fields, and the two no-load fields in place, spelling the
ratio `tap_ratio`, `tap_ratio_min`, and `tap_ratio_max`. A document written
against an earlier version that carries them under
`extras.transformer.<subtype>.<name>`, or spells the ratio `tap`, is folded
back on read; on a key collision the field already in the subtype object
wins.

OpenDSS `Xscarray` populates every winding-pair reactance in this order.
The reader applies scalar `XHL`/`XHT`/`XLT` updates at edit boundaries;
regenerated four-or-more-winding records use the complete `Xscarray`.

## ibr and control_profile

`DistIbr` and `DistControlProfile`. Both are typed and written as top-level
tables. A document written against an earlier version may carry them under
`extras`; on read they come from either place, and the top-level copy wins.

| BMOPF | PowerIO | Note |
| --- | --- | --- |
| `bus`, `terminal_map` | the same names | |
| `topology` | `topology`: `SINGLE_PHASE`, `THREE_LEG`, `FOUR_LEG` | |
| `prime_mover` | `prime_mover`: `PV`, `BATTERY`, `GENERIC`, `STATCOM` | `DSTATCOM`, removed from the schema, reads as `STATCOM`. |
| `s_max` | `s_max`, VA per phase | |
| `i_max` | `i_max`, A per conductor | |
| `p_avail` | `p_avail`, W | |
| `p_min`, `p_max`, `q_min`, `q_max` | the same names, per phase | |
| `control_profile` | `control_profile`, an id | |
| `voltage_aggregation` | `voltage_aggregation`: `PER_PHASE`, `AVERAGE` | |
| `energy_cost_rate`; read also as `cost` | `extras["energy_cost_rate"]` | Retained, not typed. The schema removed `ibr.cost`; the reader moves a stated `cost` to `energy_cost_rate` (with one `READ.BMOPF.SCHEMA_LEGACY` when the document states the current schema), a write states only `energy_cost_rate`, and the IVR objective prices the inverter from it. |
| `dc_bus`, `dc_terminal_map`, `dc_control`, `dc_v_set`, `dc_p_ref`, `dc_droop`, `dc_deadband`, `dc_link_coupled`, `p_dc_min`, `p_dc_max` | `extras` | The DC coupling fields; retained, not typed. |
| `r_filter`, `x_filter`, `b_filter_shunt`, `grid_forming`, `v_ref_internal` | `extras` | Retained, not typed. |
| `control_profile.power_factor.pf` | `PowerFactorControl` | |
| `control_profile.volt_var.*` | `VoltVarControl`: `voltage_reference`, `breakpoints`, `q_limits`, `q_unit`, `q_ref`, and the two active-power thresholds | |
| `control_profile.volt_watt.*` | `VoltWattControl`: `voltage_reference`, `breakpoints`, `p_limits`, `p_unit`, `p_ref` | |

## Fields with no typed slot

The classes below read into `MulticonductorNetwork::untyped()` by class and
name, with their properties kept as text, and are written back to the
top-level table the schema declares. A time series profile whose meaning
BMOPF's multiplicative profiles cannot carry stays under `extras.time_series`.
Reading each one reports `READ.BMOPF.RETAINED_SOURCE_ONLY`.

`dc_bus`, `dc_branch`, `dc_grounding`, `dc_load`, `dc_source`, `time_series`,
`wire_data`, `line_geometry`, and a per-element `time_series` reference map.

The DC classes, the time series and the reference maps pass through and
re-emit unchanged. `wire_data` and `line_geometry` are retained on read but
not written, each reported with `EMIT.BMOPF.RECORD_DROPPED`. Nothing reads their values into a
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
the element's own per phase rate: a generator reads it from
`DistGenerator::cost` (BMOPF `energy_cost_rate`, or its deprecated alias
`cost`), a voltage source from `VoltageSource::energy_cost_rate`, and an IBR
from its `extras["energy_cost_rate"]` (the earlier `cost` spelling when only
that is stated).
`ObjectiveTerm::NetworkGeneratorCost` is the term for a balanced network and
is not part of a BMOPF instance.

**Constraints.** `MulticonductorActiveConstraints` selects three families, each
`ConstraintSelection::All` by default, which matches the specification's rule
that constraints are active for the elements present.

| Family | The BMOPF fields it activates |
| --- | --- |
| `terminal_voltage_bounds` | `bus.vpg_min`, `vpg_max`, `vpn_min`, `vpn_max`, `vpp_min`, `vpp_max`, `vpos_min`, `vpos_max`, `vneg_max`, `vzero_max`, `vng_max` |
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

[BMOPFTools.jl](https://github.com/frederikgeth/BMOPFTools.jl) is publicly
available and is identified as the generator of `example_ieee13.json`.
The [PowerIO 0.11 compatibility change](https://github.com/frederikgeth/BMOPFTools.jl/pull/385)
exercises the typed module API, explicit legacy schema version, retained diagnostics,
transformer core-shunt locations and nominal n-winding imports. Its numerical
suite compares power-flow voltages and transformer admittance with OpenDSS,
including independently prepared BMOPF cases and native OpenDSS conversions.

PowerIO's structural tests additionally check triangular matrix completion,
conductor order, regulator fields, proposal provenance and rejected malformed
records. These checks distinguish data preservation from the equations a
particular solver supports; retaining a regulator or n-winding record does not
imply that PowerIO's own matrix compiler implements it.

## Explicit terminal-coil no-load admittance

`transformer.<subtype>.<id>.no_load_shunt` holds `{winding, g, b}` in transformer
extras and in PowerIO IR. The one-based winding index fixes its physical
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


### Per-coil capacitor arrays and regulator edits

BMOPF capacitor `q_rated` arrays state vars per coil at the stated coil voltage
`v_nom`. The reader lowers these arrays exactly into a `DistShunt` terminal
susceptance matrix: each coil contributes `q / v_nom²` to its two diagonal
entries and the negative value to the two cross entries. WYE coils use the last
mapped terminal as the return; DELTA uses the ordered cyclic pairs (one pair
for a two-terminal device). A lone WYE terminal connects to ground.

The generated name starts with `__bmopf_capacitor_` and receives a suffix if
needed to avoid a declared shunt. `READ.BMOPF.CAPACITOR_LOWERED` records this exact
representation change. Canonical emission writes the shunt; source-preserving
module emission still preserves the retained source bytes. Invalid
array shapes, negative/nonfinite ratings, nonpositive voltage, repeated terminals,
and more than 64 terminals fail parsing rather than approximating a bank.
Scalar bank capacitors continue to use `DistCapacitor`.

No hidden capacitor metadata carries electrical coefficients. Editing the
canonical shunt changes the emitted electrical data directly. Y-bus assembly,
LinDist3Flow lowering, and IVR preparation consume the same terminal matrix.
Open-delta regulators retain their original map only while it agrees with both
canonical winding maps; otherwise emission writes separate regulator legs and
reports the loss of the original grouping. N-winding `s_max` remains on its
original winding axis in the existing winding metadata.


### Scalar single-coil IBR bounds

The reader accepts scalar `p_min`, `p_max`, `q_min`, `q_max`, `s_max` and `i_max`
as one-entry vectors, matching the reference engine's single-coil convention.
In particular, `p_max: 0` is an enforced zero-availability bound, not an absent
field. Scalars are not broadcast across multiple coils; downstream cardinality
validation still applies. Malformed scalar values remain nonfinite and invalid,
rather than disappearing from the prepared problem.
