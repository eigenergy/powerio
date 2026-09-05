# PowerIO IR reference

This page lists every structural value type in the PowerIO IR, field by
field: each field's type, unit, and sign convention, the invariant the
deserializer or the constructors enforce, and what a reader uses when the
field is absent. The generated schema at `docs/schema/pio-ir/2/schema.json`
is the machine form of the same definitions. To keep the two from drifting
apart, `powerio/tests/ir_reference.rs` reads this page and checks in both
directions that each table lists the same fields the schema defines for its
definition.

## Reading the tables

Each field table sits under a line that begins with the words "Schema
definition" and gives, in backticks, the `$defs` entries of the schema the
table documents; that line is how the test pairs a table with its
definitions. The columns are:

- **field**: the JSON member name in `value.data` (or in the nested record).
- **type**: `float` is a JSON number or one of the strings `"Infinity"`,
  `"-Infinity"`, and `"NaN"`; `null` is refused at a float position.
  `float or null` is a float the source may leave unstated. `id` is a bus
  identifier: a nonnegative integer, the source's own bus number. `matrix`
  is an array of equal length float arrays. `token` is one of the listed
  strings.
- **unit**: the physical unit. `p.u.` is per unit on the network's
  `base_mva` and the bus `base_kv` unless the row says otherwise.
- **sign**: the direction a positive value means; blank when the quantity is
  unsigned or the sign carries no convention.
- **invariant**: what the deserializer refuses or the constructors hold.
- **if absent**: `required` when the serializer always writes the field and
  the deserializer refuses a document without it; otherwise the value a
  reader takes when the member is missing.

`uid` on an element row is the stable component identity that operating
points, solutions, and updates refer to. When the source gives none, `parse`
and `serialize` assign `{table}:{row}`, so a document PowerIO writes has one
wherever a table below says "assigned at serialization". `extras` on an
element is the map of source fields the typed model has no slot for, keyed
by the source's own field names; the member is required in the document but
may be empty.

## Structural type names

`value.type` is one of these names, and `value.data` has the shape of the
schema definition beside it.

| structural type | schema definition |
|---|---|
| `powerio.BalancedNetwork` | `BalancedNetwork` |
| `powerio.MulticonductorNetwork` | `MulticonductorNetwork` |
| `powerio.GeoLayer` | `GeoLayer` |
| `powerio.OperatingPoint<powerio.BalancedNetwork>` | `StoredOperatingPoint` |
| `powerio.OperatingPoint<powerio.MulticonductorNetwork>` | `StoredOperatingPoint2` |
| `powerio.TimeSeries<powerio.BalancedNetwork>` | `StoredTimeSeries` |
| `powerio.TimeSeries<powerio.MulticonductorNetwork>` | `StoredTimeSeries2` |
| `powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>` | `StoredOperatingPointTimeSeries` |
| `powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>` | `StoredOperatingPointTimeSeries2` |
| `powerio.ScenarioSet<powerio.BalancedNetwork>` | `StoredScenarioSet` |
| `powerio.ScenarioSet<powerio.MulticonductorNetwork>` | `StoredScenarioSet2` |
| `powerio.ScenarioSet<powerio.OperatingPoint<powerio.BalancedNetwork>>` | `StoredOperatingPointScenarioSet` |
| `powerio.ScenarioSet<powerio.OperatingPoint<powerio.MulticonductorNetwork>>` | `StoredOperatingPointScenarioSet2` |
| `powerio.ScenarioSet<powerio.TimeSeries<powerio.BalancedNetwork>>` | `StoredScenarioSet3` |
| `powerio.ScenarioSet<powerio.TimeSeries<powerio.MulticonductorNetwork>>` | `StoredScenarioSet4` |
| `powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.BalancedNetwork>>>` | `StoredScenarioSet5` |
| `powerio.ScenarioSet<powerio.TimeSeries<powerio.OperatingPoint<powerio.MulticonductorNetwork>>>` | `StoredScenarioSet6` |
| `powerio.DcPfInstance` | `DcPfInstance` |
| `powerio.AcPfInstance` | `AcPfInstance` |
| `powerio.DcOpfInstance` | `DcOpfInstance` |
| `powerio.AcOpfInstance` | `AcOpfInstance` |
| `powerio.McAcPfInstance` | `McAcPfInstance` |
| `powerio.McAcOpfInstance` | `McAcOpfInstance` |
| `powerio.AcScucInstance` | `AcScucInstance` |
| `powerio.DcPfSolution` | `DcPfSolution` |
| `powerio.AcPfSolution` | `AcPfSolution` |
| `powerio.DcOpfSolution` | `DcOpfSolution` |
| `powerio.AcOpfSolution` | `AcOpfSolution` |
| `powerio.SocwrOpfSolution` | `SocwrOpfSolution` |
| `powerio.McAcPfSolution` | `McAcPfSolution` |
| `powerio.McAcOpfSolution` | `McAcOpfSolution` |
| `powerio.AcScucSolution` | `AcScucSolution` |

## powerio.BalancedNetwork

The positive sequence transmission model, in the units MATPOWER uses: powers
in MW and MVAr, voltage magnitudes in per unit, angles in degrees, and
impedances in per unit on `base_mva`. Bus identifiers are the source's own.

Schema definition: `BalancedNetwork`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | | required |
| `base_mva` | float | MVA | | finite and positive | required |
| `base_frequency` | float | Hz | | positive | 60 |
| `source_format` | token | | | one of the format tokens (`matpower`, `psse`, `psse-rawx`, `powermodels-json`, `egret-json`, `powerworld`, `powerworld-pwb`, `pslf`, `pandapower-json`, `pypsa-csv`, `gridfm`, `goc3-json`, `surge-json`, `opfdata-json`, `xiidm`, `cgmes`, `in-memory`, `normalized`) | required |
| `buses` | array of `Bus` | | | `id` unique | required |
| `loads` | array of `Load` | | | `bus` names a bus | required |
| `shunts` | array of `Shunt` | | | `bus` names a bus | required |
| `branches` | array of `Branch` | | | `from` and `to` name buses | required |
| `generators` | array of `Generator` | | | `bus` names a bus | required |
| `storage` | array of `Storage` | | | `bus` names a bus | required |
| `hvdc` | array of `Hvdc` | | | `from` and `to` name buses | required |
| `switches` | array of `Switch` | | | `from` and `to` name buses | `[]` |
| `transformers_3w` | array of `Transformer3W` | | | every winding `bus` names a bus | `[]` |
| `static_var_compensators` | array of `StaticVarCompensator` | | | `bus` names a bus | `[]` |
| `areas` | array of `Area` | | | `number` unique | `[]` |
| `solver` | `SolverParams` or null | | | | null |
| `case_metadata` | `CaseMetadata` | | | | every member null |
| `geo` | `GeoMeta` or null | | | | null |
| `detailed_connectivity` | `DetailedConnectivity` or null | | | | null |
| `generated_uids` | array of string | | | subset of component `uid` values; identifies identities PowerIO assigned because the source stated none | `[]` |

### Bus

Schema definition: `Bus`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | id | | | unique within `buses` | required |
| `kind` | token `PQ`, `PV`, `REF`, `ISOLATED` | | | MATPOWER type codes 1, 2, 3, 4 | required |
| `vm` | float | p.u. | | | required |
| `va` | float | degrees | | | required |
| `base_kv` | float | kV | | nonnegative; 0 when the source states none | required |
| `vmax` | float | p.u. | | `vmin <= vmax`; `Infinity` for no bound | required |
| `vmin` | float | p.u. | | | required |
| `evhi` | float or null | p.u. | | emergency band, stated only when it differs from `vmax` | null (equals `vmax`) |
| `evlo` | float or null | p.u. | | stated only when it differs from `vmin` | null (equals `vmin`) |
| `area` | integer | | | | required |
| `zone` | integer | | | | required |
| `name` | string or null | | | | null |
| `uid` | string or null | | | unique within `buses` | assigned at serialization |
| `location` | `Location` or null | network coordinate space | | | null |
| `extras` | object | | | | required |

### Load

Schema definition: `Load`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | id | | | names a bus | required |
| `p` | float | MW | positive is consumption | | required |
| `q` | float | MVAr | positive is inductive consumption | | required |
| `in_service` | boolean | | | | required |
| `voltage_model` | `LoadVoltageModel` or null | | | | null (constant power) |
| `uid` | string or null | | | unique within `loads` | assigned at serialization |
| `extras` | object | | | | required |

`LoadVoltageModel` is tagged by `kind`. `constant_power` has no further
member. `zip` has `p_constant_power`, `p_constant_current`,
`p_constant_impedance` (MW, summing to `p`) and `q_constant_power`,
`q_constant_current`, `q_constant_impedance` (MVAr, summing to `q`), plus an
optional `v_nom` (kV), an optional source `load_type` code, and an optional
`scaling` factor. `exponential` has `gamma_p`, `gamma_q`, and `v_nom`, with
`P = p (V / v_nom)^gamma_p` and `Q = q (V / v_nom)^gamma_q`.

### Shunt

Schema definition: `Shunt`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | id | | | names a bus | required |
| `g` | float | MW at V = 1 p.u. | positive is consumption | | required |
| `b` | float | MVAr at V = 1 p.u. | positive is capacitive injection | the initial value of a switched shunt | required |
| `in_service` | boolean | | | | required |
| `section_count` | integer or null | | | | null (unset) |
| `control` | `SwitchedShuntControl` or null | | | | null (fixed shunt) |
| `uid` | string or null | | | unique within `shunts` | assigned at serialization |
| `extras` | object | | | | required |

Schema definition: `SwitchedShuntControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `mode` | token `locked`, `continuous`, `discrete` | | | PSS/E `MODSW` 0, 1, 2 and up | required |
| `vhigh` | float | p.u. | | `vlow <= vhigh` | required |
| `vlow` | float | p.u. | | | required |
| `control_bus` | id or null | | | names a bus | null (the shunt's own bus) |
| `regulating_terminal` | `TerminalReference` or null | | | | null |
| `rmpct` | float | percent | | | required |
| `blocks` | array of `ShuntBlock` | | | | required |

Schema definition: `ShuntBlock`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `steps` | integer | | | | required |
| `g` | float | MW at V = 1 p.u. per step | positive is consumption | | required |
| `b` | float | MVAr at V = 1 p.u. per step | positive is capacitive injection | | required |

### Branch

Schema definition: `Branch`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `from` | id | | | names a bus | required |
| `to` | id | | | names a bus | required |
| `r` | float | p.u. | | | required |
| `x` | float | p.u. | | `r` and `x` not both zero when a matrix is built | required |
| `b` | float | p.u. | positive is capacitive | total line charging; half at each end unless `charging` is present | required |
| `charging` | `BranchCharging` or null | | | canonical per terminal admittance when present | null (derive from `b`) |
| `rate_a` | float | MVA | | 0 is unrated | required |
| `rate_b` | float | MVA | | 0 is unrated | required |
| `rate_c` | float | MVA | | 0 is unrated | required |
| `rating_sets` | array of `BranchRatingSet` | | | | `[]` |
| `current_ratings` | `BranchCurrentRatings` or null | | | | null |
| `tap` | float | ratio at the from side | | 0 means 1 (a line); otherwise positive | required |
| `shift` | float | degrees | positive means the from side voltage leads the to side | | required |
| `in_service` | boolean | | | | required |
| `angmin` | float | degrees | | `angmin <= angmax`; -360 and 360 mean unconstrained | required |
| `angmax` | float | degrees | | | required |
| `control` | `TransformerControl` or null | | | | null (a line or a fixed ratio transformer) |
| `solution` | `BranchSolution` or null | | | | null |
| `name` | string or null | | | | null |
| `uid` | string or null | | | unique within `branches` | assigned at serialization |
| `route` | array of `Location` or null | network coordinate space | | | null |
| `extras` | object | | | | required |

Schema definition: `BranchCharging`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `g_fr` | float | p.u. | | | required |
| `b_fr` | float | p.u. | positive is capacitive | | required |
| `g_to` | float | p.u. | | | required |
| `b_to` | float | p.u. | positive is capacitive | | required |

Schema definition: `BranchRatingSet`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | | required |
| `rate_mva` | float | MVA | | | required |

Schema definition: `BranchCurrentRatings`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `c_rating_a` | float | source units (amperes for PSS/E) | | | required |
| `c_rating_b` | float | source units | | | required |
| `c_rating_c` | float | source units | | | required |

Schema definition: `BranchSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `pf` | float | MW | positive flows into the branch at the from terminal | | required |
| `qf` | float | MVAr | positive flows into the branch at the from terminal | | required |
| `pt` | float | MW | positive flows into the branch at the to terminal | | required |
| `qt` | float | MVAr | positive flows into the branch at the to terminal | | required |

Schema definition: `TransformerControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `mode` | token `fixed`, `voltage`, `reactive_flow`, `active_flow`, `dc_line_quantity`, `asymmetric_active_flow` | | | PSS/E `COD` magnitude 0 through 5 | required |
| `enabled` | boolean | | | automatic adjustment on; the sign of PSS/E `COD` | required |
| `controlled_bus` | id or null | | | names a bus | null |
| `controlled_bus_on_winding_side` | boolean | | | | false |
| `regulating_terminal` | `TerminalReference` or null | | | | null |
| `band_min` | float | p.u. voltage, MVAr, or MW as `mode` selects | | `band_min <= band_max` | required |
| `band_max` | float | as `band_min` | | | required |
| `tap_min` | float | ratio, or degrees for phase control | | `tap_min <= tap_max` | required |
| `tap_max` | float | as `tap_min` | | | required |
| `ntp` | integer | | | number of tap positions | required |
| `mva_base` | float | MVA | | | required |
| `winding_connection_angle` | float or null | degrees | | asymmetric active power flow control only | null |

### Generator

Schema definition: `Generator`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | id | | | names a bus | required |
| `pg` | float | MW | positive is generation | | required |
| `qg` | float | MVAr | positive is generation | | required |
| `qmax` | float | MVAr | | `qmin <= qmax`; `Infinity` and `-Infinity` mean unbounded | required |
| `qmin` | float | MVAr | | | required |
| `vg` | float | p.u. | | | required |
| `mbase` | float | MVA | | positive when stated | required |
| `pmax` | float | MW | | `pmin <= pmax` | required |
| `pmin` | float | MW | | | required |
| `in_service` | boolean | | | | required |
| `cost` | `GenCost` or null | | | | null (no cost curve) |
| `caps` | map of string to float | MW, MVAr, MW per minute, or a fraction per key | | keys among `pc1`, `pc2`, `qc1min`, `qc1max`, `qc2min`, `qc2max`, `ramp_agc`, `ramp_10`, `ramp_30`, `ramp_q`, `apf`, the MATPOWER columns past `PMIN` | `{}` |
| `energy_source` | token `hydro`, `nuclear`, `wind`, `thermal`, `solar`, `other` | | | | `other` |
| `voltage_regulation_on` | boolean | | | | true |
| `regulated_bus` | id or null | | | names a bus | null (the generator's own bus) |
| `regulating_terminal` | `TerminalReference` or null | | | | null |
| `active_power_control` | `ActivePowerControl` or null | | | | null |
| `uid` | string or null | | | unique within `generators` | assigned at serialization |

Schema definition: `GenCost`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `model` | integer | | | 1 is piecewise linear, 2 is polynomial | required |
| `startup` | float | currency | | | required |
| `shutdown` | float | currency | | | required |
| `ncost` | integer | | | polynomial: `coeffs.len() == ncost`; piecewise: `coeffs.len() == 2 * ncost` | required |
| `coeffs` | array of float | polynomial: currency per MW^k per hour, highest order first; piecewise: alternating MW and currency per hour breakpoints | | | required |

Schema definition: `ActivePowerControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `participate` | boolean | | | | required |
| `droop_percent` | float or null | percent | | | null |
| `participation_factor` | float or null | | | | null |
| `minimum_target_active_power_mw` | float or null | MW | | | null |
| `maximum_target_active_power_mw` | float or null | MW | | | null |

### Storage

The PowerModels `storage` model.

Schema definition: `Storage`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | id | | | names a bus | required |
| `ps` | float | MW | positive is withdrawn from the network (charging) | | required |
| `qs` | float | MVAr | positive is withdrawn from the network | | required |
| `energy` | float | MWh | | `0 <= energy <= energy_rating` | required |
| `energy_rating` | float | MWh | | | required |
| `charge_rating` | float | MW | | | required |
| `discharge_rating` | float | MW | | | required |
| `charge_efficiency` | float | fraction | | in `[0, 1]` | required |
| `discharge_efficiency` | float | fraction | | in `[0, 1]` | required |
| `thermal_rating` | float | MVA | | | required |
| `current_rating` | float or null | amperes | | | null |
| `qmin` | float | MVAr | | `qmin <= qmax` | required |
| `qmax` | float | MVAr | | | required |
| `r` | float | p.u. | | | required |
| `x` | float | p.u. | | | required |
| `p_loss` | float | MW | | standby loss | required |
| `q_loss` | float | MVAr | | standby loss | required |
| `in_service` | boolean | | | | required |
| `active_power_control` | `ActivePowerControl` or null | | | | null |
| `uid` | string or null | | | unique within `storage` | assigned at serialization |
| `extras` | object | | | | required |

### Hvdc

A two terminal HVDC line in the MATPOWER `dcline` convention, whatever the
source format.

Schema definition: `Hvdc`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `from` | id | | | names a bus | required |
| `to` | id | | | names a bus | required |
| `in_service` | boolean | | | | required |
| `pf` | float | MW | positive flows from the from bus to the to bus, measured at the from end | | required |
| `pt` | float | MW | positive flows from the from bus to the to bus, measured at the to end | | required |
| `qf` | float | MVAr | positive is injected into the from bus | | required |
| `qt` | float | MVAr | positive is injected into the to bus | | required |
| `vf` | float | p.u. | | voltage setpoint at the from bus | required |
| `vt` | float | p.u. | | voltage setpoint at the to bus | required |
| `pmin` | float | MW | | bounds on `pf`; `pmin <= pmax` | required |
| `pmax` | float | MW | | | required |
| `qminf` | float | MVAr | | bounds on `qf` | required |
| `qmaxf` | float | MVAr | | | required |
| `qmint` | float | MVAr | | bounds on `qt` | required |
| `qmaxt` | float | MVAr | | | required |
| `loss0` | float | MW | | constant loss term | required |
| `loss1` | float | MW per MW | | linear loss term in `pf` | required |
| `resistance_ohm` | float or null | ohm | | | null |
| `nominal_voltage_kv` | float or null | kV | | | null |
| `converters_mode` | token `side1_rectifier_side2_inverter`, `side1_inverter_side2_rectifier`, or null | | | | null |
| `converter1` | `HvdcConverter` or null | | | | null |
| `converter2` | `HvdcConverter` or null | | | | null |
| `cost` | `GenCost` or null | | | usage cost in `pf` | null |
| `uid` | string or null | | | unique within `hvdc` | assigned at serialization |
| `extras` | object | | | | required |

Schema definition: `HvdcConverter`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `component` | `ComponentId` | | | | required |
| `kind` | token `vsc`, `lcc` | | | | required |
| `loss_factor_percent` | float | percent of active power | | | required |
| `power_factor` | float or null | | | | null |
| `reactive_power_setpoint_mvar` | float or null | MVAr | | | null |
| `regulating_terminal` | `TerminalReference` or null | | | | null |
| `voltage_regulator_on` | boolean or null | | | | null |
| `voltage_setpoint_kv` | float or null | kV | | | null |

### Switch

A transmission switch. A closed switch stays a switch in the data; the matrix
calculations do not lower it to a zero impedance branch.

Schema definition: `Switch`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `from` | id | | | names a bus | required |
| `to` | id | | | names a bus | required |
| `closed` | boolean | | | | required |
| `thermal_rating` | float or null | MVA | | | null |
| `current_rating` | float or null | amperes | | | null |
| `pf` | float or null | MW | positive flows into the switch at the from terminal | | null |
| `qf` | float or null | MVAr | positive flows into the switch at the from terminal | | null |
| `pt` | float or null | MW | positive flows into the switch at the to terminal | | null |
| `qt` | float or null | MVAr | positive flows into the switch at the to terminal | | null |
| `uid` | string or null | | | unique within `switches` | assigned at serialization |
| `extras` | object | | | | required |

### Transformer3W

Three windings joined at a star point; the indexed view star-lowers the
record for matrix calculations.

Schema definition: `Transformer3W`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string or null | | | | null |
| `windings` | array of `Winding` | | | exactly three, in the order primary, secondary, tertiary | required |
| `z` | array of `Impedance` | | | exactly three: `z12`, `z23`, `z31` | required |
| `mag_g` | float | p.u. on the system base, at the star point | | | required |
| `mag_b` | float | p.u. on the system base, at the star point | positive is capacitive | | required |
| `star_vm` | float | p.u. | | solved star point voltage | required |
| `star_va` | float | degrees | | | required |
| `in_service` | boolean | | | | required |
| `uid` | string or null | | | unique within `transformers_3w` | assigned at serialization |
| `extras` | object | | | | required |

Schema definition: `Winding`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | id | | | names a bus | required |
| `nominal_kv` | float | kV | | 0 defers to the terminal bus `base_kv` | required |
| `tap` | float | ratio | | 1 is nominal (PSS/E `WINDV` with `CW = 1`) | required |
| `shift` | float | degrees | as `Branch.shift` | | required |
| `rate_a` | float | MVA | | 0 is unrated | required |
| `rate_b` | float | MVA | | | required |
| `rate_c` | float | MVA | | | required |
| `control` | `TransformerControl` or null | | | | null |

Schema definition: `Impedance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `r` | float | p.u. on the system base | | | required |
| `x` | float | p.u. on the system base | | | required |
| `base_mva` | float | MVA | | the source's declared base for the pair; positive | required |

### Area

Schema definition: `Area`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `number` | integer | | | unique within `areas`; matches `Bus.area` | required |
| `name` | string or null | | | | null |
| `net_interchange` | float | MW | positive is export out of the area | | required |
| `tolerance` | float | MW | | | required |
| `slack_bus` | id or null | | | names a bus | null |
| `area_type` | string or null | | | | null |
| `uid` | string or null | | | | null |

### StaticVarCompensator

Schema definition: `StaticVarCompensator`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | id | | | names a bus | required |
| `b_min_siemens` | float | siemens | | `b_min_siemens <= b_max_siemens` | required |
| `b_max_siemens` | float | siemens | | | required |
| `voltage_setpoint_kv` | float | kV | | | required |
| `reactive_power_setpoint_mvar` | float | MVAr | positive is injection | | required |
| `regulation_mode` | token `voltage`, `reactive_power` | | | | required |
| `regulating` | boolean | | | | required |
| `regulating_terminal` | `TerminalReference` or null | | | | null |
| `p` | float | MW | positive is consumption | | required |
| `q` | float | MVAr | positive is consumption | | required |
| `in_service` | boolean | | | | required |
| `uid` | string or null | | | | null |
| `extras` | object | | | | required |

### SolverParams

Each member is set only when the source has it.

Schema definition: `SolverParams`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `newton_tolerance` | float or null | MW and MVAr mismatch | | | null |
| `max_iterations` | integer or null | | | | null |
| `zero_impedance_threshold` | float or null | p.u. reactance | | | null |
| `adjust_taps` | boolean or null | | | | null |
| `adjust_phase_shift` | boolean or null | | | | null |
| `adjust_dc_taps` | boolean or null | | | | null |
| `adjust_switched_shunt` | boolean or null | | | | null |
| `adjust_area_interchange` | boolean or null | | | | null |

### CaseMetadata

Schema definition: `CaseMetadata`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `case_date` | string or null | | | | null |
| `forecast_distance` | integer or null | minutes, as the source states | | | null |
| `source_model_format` | string or null | | | | null |
| `minimum_validation_level` | string or null | | | | null |

### Coordinates

Schema definition: `GeoMeta`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `kind` | token `source`, `synthetic`, `manual`, `derived`, or null | | | default origin of points without their own `kind` | null |

Schema definition: `Location`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `x` | float | network coordinate space; longitude in geographic space | | | required |
| `y` | float | network coordinate space; latitude in geographic space | | | required |
| `kind` | token as `GeoMeta.kind`, or null | | | | null (the network default) |

### Component references

Schema definition: `ComponentId`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `component_type` | string | | | the element table or record kind | required |
| `local_id` | string | | | the source supplied or assigned identity | required |

Schema definition: `TerminalReference`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `equipment` | `ComponentId` | | | | required |
| `terminal` | integer | | | 1 for single terminal equipment, else the side number | required |

### DetailedConnectivity

The hierarchy and bus breaker or node breaker connectivity a source gives
beyond the balanced calculation view (XIIDM, CGMES, PSS/E RAW 35 and RAWX).
Every collection is empty when absent. The schema defines the record types
under the names below. Their field names end in their units (`_kv`, `_mw`,
`_mvar`, `_a`, `_ohm`, `_h`, `_f`, `_km`, `_degrees`, `_percent`,
`_seconds`), and terminal powers use the load sign convention (positive is
consumption) where the record says so.

Schema definition: `DetailedConnectivity`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `subnetworks` | array of `Subnetwork` | | | | `[]` |
| `substations` | array of `Substation` | | | | `[]` |
| `voltage_levels` | array of `VoltageLevel` | kV | | `nominal_kv` positive | `[]` |
| `bus_breaker_buses` | array of `BusBreakerBus` | | | | `[]` |
| `calculated_buses` | array of `CalculatedBus` | | | | `[]` |
| `connectivity_nodes` | array of `ConnectivityNode` | | | | `[]` |
| `busbar_sections` | array of `BusbarSection` | | | | `[]` |
| `junctions` | array of `Junction` | | | | `[]` |
| `terminals` | array of `Terminal` | | | | `[]` |
| `switches` | array of `TopologySwitch` | | | | `[]` |
| `internal_connections` | array of `InternalConnection` | | | | `[]` |
| `operational_limit_groups` | array of `OperationalLimitGroup` | amperes, MW, or MVA per limit kind | | | `[]` |
| `tap_changers` | array of `TapChanger` | | | `low_tap_position <= tap_position` when stated | `[]` |
| `equipment_reactive_limits` | array of `EquipmentReactiveLimits` | MVAr | | | `[]` |
| `boundary_lines` | array of `BoundaryLine` | | | | `[]` |
| `tie_lines` | array of `TieLine` | | | | `[]` |
| `component_metadata` | array of `ComponentMetadata` | | | | `[]` |
| `omitted_fields` | array of `OmittedField` | | | a field absent from the source, distinct from a stated zero | `[]` |
| `dc_converter_units` | array of `DcConverterUnit` | | | | `[]` |
| `dc_topological_nodes` | array of `DcTopologicalNode` | | | | `[]` |
| `dc_nodes` | array of `DcNode` | | | | `[]` |
| `dc_grounds` | array of `DcGround` | | | | `[]` |
| `dc_busbars` | array of `DcBusbar` | | | | `[]` |
| `dc_lines` | array of `DcLine` | | | | `[]` |
| `dc_series_devices` | array of `DcSeriesDevice` | | | | `[]` |
| `dc_switches` | array of `DcSwitch` | | | | `[]` |
| `voltage_source_converters` | array of `VoltageSourceConverter` | | | | `[]` |
| `line_commutated_converters` | array of `LineCommutatedConverter` | | | | `[]` |

## powerio.MulticonductorNetwork

The conductor level distribution model, in SI units (watts, vars, volts,
amperes, ohms, siemens, meters) with angles in radians. Bus identifiers and
terminal names are the source's own strings; for OpenDSS the terminal names
are its node numbers. An element's `terminal_map` lists, in conductor order,
the terminals of the named bus it connects to.

Schema definition: `MulticonductorNetwork`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string or null | | | | null |
| `base_frequency` | float | Hz | | positive | required |
| `source_format` | token `dss`, `bmopf-json`, `pmd-json`, or null | | | | null |
| `buses` | array of `DistBus` | | | `id` unique | required |
| `linecodes` | array of `DistLineCode` | | | `name` unique | required |
| `lines` | array of `DistLine` | | | `bus_from`, `bus_to` name buses; `linecode` names a line code | required |
| `switches` | array of `DistSwitch` | | | `bus_from`, `bus_to` name buses | required |
| `transformers` | array of `DistTransformer` | | | every winding `bus` names a bus | required |
| `loads` | array of `DistLoad` | | | `bus` names a bus | required |
| `shunts` | array of `DistShunt` | | | `bus` names a bus | required |
| `capacitors` | array of `DistCapacitor` | | | `bus` names a bus | `[]` |
| `generators` | array of `DistGenerator` | | | `bus` names a bus | required |
| `ibrs` | array of `DistIbr` | | | `bus` names a bus; `control_profile` names a profile | `[]` |
| `control_profiles` | array of `DistControlProfile` | | | `name` unique | `[]` |
| `sources` | array of `VoltageSource` | | | BMOPF carries exactly one | required |
| `untyped` | array of `UntypedObject` | | | | required |
| `commands` | array of `[verb, args]` string pairs | | | source order | required |
| `options` | array of `[name, value]` string pairs | | | source order | required |
| `extras` | object | | | | required |
| `geo` | `DistGeoMeta` or null | | | | null |

### DistBus

Schema definition: `DistBus`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | string | | | unique within `buses` | required |
| `terminals` | array of string | | | ordered, unique | required |
| `grounded` | array of string | | | each names a terminal of this bus; zero impedance to ground | required |
| `v_min` | float or null | volts | | `v_min <= v_max` | null (unbounded) |
| `v_max` | float or null | volts | | | null (unbounded) |
| `vpn_min` | array of float or null | volts | | per phase to neutral bound | null |
| `vpn_max` | array of float or null | volts | | | null |
| `vpp_min` | array of float or null | volts | | per phase to phase bound | null |
| `vpp_max` | array of float or null | volts | | | null |
| `vpos_min` | float or null | volts | | positive sequence bound | null |
| `vpos_max` | float or null | volts | | | null |
| `vneg_max` | float or null | volts | | negative sequence magnitude cap | null |
| `vzero_max` | float or null | volts | | zero sequence magnitude cap | null |
| `vn_max` | float or null | volts | | neutral to ground magnitude cap | null |
| `location` | `DistLocation` or null | network coordinate space | | | null |
| `extras` | object | | | | required |

### DistLineCode

Schema definition: `DistLineCode`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `linecodes` | required |
| `n_conductors` | integer | | | positive; the order of every matrix | required |
| `r_series` | matrix | ohm per meter | | `n_conductors` square, symmetric | required |
| `x_series` | matrix | ohm per meter | | `n_conductors` square, symmetric | required |
| `g_from` | matrix | siemens per meter | | half of the shunt admittance, at the from end | required |
| `b_from` | matrix | siemens per meter | positive is capacitive | | required |
| `g_to` | matrix | siemens per meter | | half of the shunt admittance, at the to end | required |
| `b_to` | matrix | siemens per meter | positive is capacitive | | required |
| `i_max` | array of float or null | amperes per conductor | | | null |
| `s_max` | array of float or null | VA per conductor | | | null |
| `source` | string or null | | | origin of the matrices (BMOPF `source`) | null |
| `extras` | object | | | | required |

### DistLine

Schema definition: `DistLine`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `lines` | required |
| `bus_from` | string | | | names a bus | required |
| `bus_to` | string | | | names a bus | required |
| `terminal_map_from` | array of string | | | `n_conductors` terminals of `bus_from` | required |
| `terminal_map_to` | array of string | | | `n_conductors` terminals of `bus_to` | required |
| `linecode` | string | | | names a line code | required |
| `length` | float | meters | | nonnegative | required |
| `i_max` | array of float or null | amperes per conductor | | | null (the line code's) |
| `s_max` | array of float or null | VA per conductor | | | null (the line code's) |
| `route` | array of `DistLocation` or null | network coordinate space | | | null |
| `extras` | object | | | | required |

### DistSwitch

Schema definition: `DistSwitch`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `switches` | required |
| `bus_from` | string | | | names a bus | required |
| `bus_to` | string | | | names a bus | required |
| `terminal_map_from` | array of string | | | terminals of `bus_from`, same length as `terminal_map_to` | required |
| `terminal_map_to` | array of string | | | terminals of `bus_to` | required |
| `open` | boolean | | | | required |
| `i_max` | array of float or null | amperes per conductor | | | null |
| `extras` | object | | | | required |

### DistTransformer

Schema definition: `DistTransformer`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `transformers` | required |
| `phases` | integer | | | 1 through 3 | required |
| `windings` | array of `DistWinding` | | | two or three | required |
| `xsc_pct` | array of float | percent | | `[xhl]` for two windings, `[xhl, xht, xlt]` for three | required |
| `extras` | object | | | | required |

Schema definition: `DistWinding`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus` | required |
| `conn` | token `wye`, `delta` | | | | required |
| `v_ref` | float | volts, line to line for two and three phases | | positive | required |
| `s_rating` | float | VA | | positive | required |
| `r_pct` | float | percent of the winding base | | | required |
| `tap` | float | ratio | | 1 is nominal | required |
| `r_neutral` | float or null | ohm | | | null |
| `x_neutral` | float or null | ohm | | | null |

### DistLoad

Schema definition: `DistLoad`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `loads` | required |
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus` | required |
| `configuration` | token `wye`, `delta`, `single_phase` | | | | required |
| `p_nom` | array of float | watts per phase | positive is consumption | one entry per active phase | required |
| `q_nom` | array of float | vars per phase | positive is inductive consumption | same length as `p_nom` | required |
| `voltage_model` | `DistLoadVoltageModel` | | | | required |
| `extras` | object | | | | required |

`DistLoadVoltageModel` is tagged by `model`. `constant_power`,
`constant_current`, and `constant_impedance` each have `v_nom` (volts per
active phase). `zip` has `v_nom` and the per phase coefficient arrays
`alpha_z`, `alpha_i`, `alpha_p` (active power) and `beta_z`, `beta_i`,
`beta_p` (reactive power), each triple summing to one. `exponential` has
`v_nom`, `gamma_p`, and `gamma_q`.

### DistCapacitor

Schema definition: `DistCapacitor`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `capacitors` | required |
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus` | required |
| `configuration` | token `wye`, `delta`, `single_phase` | | | | required |
| `q_rated` | float | vars, whole bank at `v_nom` | positive is capacitive injection | | required |
| `v_nom` | float | volts, line to line for the three phase configurations | | positive | required |
| `extras` | object | | | | required |

### DistShunt

Schema definition: `DistShunt`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `shunts` | required |
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus`, the order of the matrices | required |
| `g` | matrix | siemens | | square in `terminal_map` | required |
| `b` | matrix | siemens | positive is capacitive | square in `terminal_map` | required |
| `extras` | object | | | | required |

### DistGenerator

Schema definition: `DistGenerator`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `generators` | required |
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus` | required |
| `configuration` | token `wye`, `delta`, `single_phase` | | | | required |
| `p_nom` | array of float | watts per phase | positive is generation | | required |
| `q_nom` | array of float | vars per phase | positive is generation | same length as `p_nom` | required |
| `p_min` | array of float or null | watts per phase | | `p_min <= p_max` | null |
| `p_max` | array of float or null | watts per phase | | | null |
| `q_min` | array of float or null | vars per phase | | `q_min <= q_max` | null |
| `q_max` | array of float or null | vars per phase | | | null |
| `s_max` | array of float or null | VA per conductor | | | null |
| `i_max` | array of float or null | amperes per conductor | | | null |
| `cost` | array of float or null | currency per kWh per phase, or one scalar | | | null |
| `extras` | object | | | | required |

### DistIbr

Schema definition: `DistIbr`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `ibrs` | required |
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus` | required |
| `prime_mover` | token `PV`, `BATTERY`, `GENERIC`, `STATCOM`, `DSTATCOM` | | | | required |
| `topology` | token `SINGLE_PHASE`, `THREE_LEG`, `FOUR_LEG` | | | | required |
| `s_max` | array of float | VA per phase | | nameplate | required |
| `p_avail` | float or null | watts | | | null |
| `p_min` | array of float or null | watts per phase | | | null |
| `p_max` | array of float or null | watts per phase | | | null |
| `q_min` | array of float or null | vars per phase | | | null |
| `q_max` | array of float or null | vars per phase | | | null |
| `i_max` | array of float or null | amperes per conductor | | | null |
| `voltage_aggregation` | token `PER_PHASE`, `AVERAGE`, or null | | | | null |
| `control_profile` | string or null | | | names a control profile | null |
| `extras` | object | | | | required |

### DistControlProfile

Schema definition: `DistControlProfile`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `control_profiles` | required |
| `volt_var` | `VoltVarControl` or null | | | | null |
| `volt_watt` | `VoltWattControl` or null | | | | null |
| `power_factor` | `PowerFactorControl` or null | | | | null |
| `extras` | object | | | | required |

Schema definition: `VoltVarControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `breakpoints` | array of float | p.u. voltage | | ascending | required |
| `q_limits` | array of float | `q_unit` | positive is injection | same length as `breakpoints` | required |
| `voltage_reference` | token `PN_PER_PHASE`, `PP_PER_PHASE`, `PP_AVERAGED`, `PG_AVERAGED`, `PN_AVERAGED`, `PG_PER_PHASE`, or null | | | | null |
| `q_ref` | token `VAR_MAX`, `VAR_AVAILABLE`, or null | | | | null |
| `q_unit` | token `VA_FRACTION`, `VAR`, or null | | | | null |
| `p_min_for_q` | float or null | watts | | | null |
| `p_min_for_q_max` | float or null | watts | | | null |

Schema definition: `VoltWattControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `breakpoints` | array of float | p.u. voltage | | ascending | required |
| `p_limits` | array of float | `p_unit` | | same length as `breakpoints` | required |
| `voltage_reference` | token as `VoltVarControl.voltage_reference`, or null | | | | null |
| `p_ref` | token `P_AVAILABLE`, `P_MAX`, `S_MAX`, or null | | | | null |
| `p_unit` | token `VA_FRACTION`, `W`, or null | | | | null |

Schema definition: `PowerFactorControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `pf` | float | | | in `[-1, 1]` | required |

### VoltageSource

Schema definition: `VoltageSource`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | unique within `sources` | required |
| `bus` | string | | | names a bus | required |
| `terminal_map` | array of string | | | terminals of `bus` | required |
| `v_magnitude` | array of float | volts per terminal | | 0 on a grounded terminal; same length as `terminal_map` | required |
| `v_angle` | array of float | radians per terminal | | same length as `terminal_map` | required |
| `extras` | object | | | | required |

### UntypedObject

An object the reader recognized but does not type, kept so a conversion can
report it precisely.

Schema definition: `UntypedObject`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `class` | string | | | | required |
| `name` | string | | | | required |
| `props` | array of `[key, value]` string pairs | | | source order | required |

### Distribution coordinates

Schema definition: `DistGeoMeta`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `kind` | token `source`, `synthetic`, `manual`, `derived`, or null | | | | null |

Schema definition: `DistLocation`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `x` | float | network coordinate space; longitude in geographic space | | | required |
| `y` | float | network coordinate space; latitude in geographic space | | | required |
| `kind` | token as `DistGeoMeta.kind`, or null | | | | null (the network default) |

## powerio.GeoLayer

A standalone geographic document: element points and routes in one coordinate
space, keyed by element identity rather than embedded in a case. The
canonical `.geo.json`, GeoJSON, aliased CSV or JSON records, headerless
buscoords CSV, and a PowerWorld `.pwd` display all parse to one, and
`apply_geo_layer` places it onto a network.

Schema definition: `GeoLayer`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `space` | `CoordinateSpace` | | | the space every feature's coordinates are in | required |
| `kind` | token `source`, `synthetic`, `manual`, `derived`, or null | | | default origin of features without their own `kind` | null |
| `features` | array of `GeoFeature` | | | | empty |

`CoordinateSpace` is tagged by `space`. `geographic` has an optional `crs`;
x is longitude and y latitude in decimal degrees, and a null `crs` means
EPSG:4326. `projected` has an optional `crs` for planar coordinates.
`diagram` has an optional `canvas` for drawing coordinates with no earth
referent. `unknown` has no further member and means the source declared no
space.

Schema definition: `Canvas`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `width` | float or null | canvas units | positive | the drawing width the source states | null |
| `height` | float or null | canvas units | positive | the drawing height the source states | null |
| `units` | string or null | | | the source's own name for its canvas units | null |

### GeoFeature

Schema definition: `GeoFeature`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `target` | token `bus`, `branch`, or `substation` | | | the element family the feature places | required |
| `key` | `ElementKey` | | | at least one member names an element, unless a branch states both endpoints | required |
| `geometry` | `GeoGeometry` | | | every coordinate is finite | required |
| `from` | string or null | | | a branch's endpoint bus, the unordered fallback identity | null |
| `to` | string or null | | | the other endpoint bus | null |
| `kind` | token as `GeoLayer.kind`, or null | | | | null (the layer default) |

`GeoGeometry` is one tagged object: `point` is a single `[x, y]` position for
a placed element, and `line_string` is an array of `[x, y]` positions for a
route. Positions are in the layer's coordinate space, a route has at least
one, and every coordinate is finite.

### ElementKey

Matching tries `uid`, then `id`, then case insensitive `name`; a branch
additionally falls back to the unordered `(from, to)` bus pair.

Schema definition: `ElementKey`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `uid` | string or null | | | the durable identity (`buses:3`, `branches:7`) | null |
| `id` | string or null | | | the source's own element identifier | null |
| `name` | string or null | | | matched case insensitively | null |
| `index` | integer or null | | positive | 1-based row alias, accepted on read and never written | null |

## powerio.OperatingPoint\<N\>

An alternate electrical assignment, possibly partial, over the fixed
component identities of one base network `N`: demand, setpoints, dispatch,
voltages, injections, service status, switch positions, transformer taps, and
phase shifts. Any quantity the point leaves out resolves to the network's own
value.

Schema definitions: `StoredOperatingPoint`, `StoredOperatingPoint2`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `BalancedNetwork` or `MulticonductorNetwork`, as the type name says | | | the complete base network | required |
| `quantities` | map of quantity name to `StoredQuantity` | per quantity, below | | keys among the quantity names of the network family | required |

Schema definition: `StoredQuantity`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `identities` | array of string | | | each names a component of the network; unique; same length as `values` | required |
| `values` | array of float | the quantity's unit; a flag is 0 or 1 | | | required |

An operating point stored inside a collection or an instance omits the
network, because the enclosing record gives it once.

Schema definition: `StoredOperatingPointAssignment`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `quantities` | map of quantity name to `StoredQuantity` | | | as `StoredOperatingPoint.quantities` | required |

Balanced quantities. A bus quantity is keyed by the bus id's decimal
spelling; an element quantity by the element's `uid`.

| quantity | keyed by | unit | sign |
|---|---|---|---|
| `bus_voltage_magnitude` | bus id | p.u. | |
| `bus_voltage_angle` | bus id | degrees | |
| `bus_active_injection` | bus id | MW | positive into the network |
| `bus_reactive_injection` | bus id | MVAr | positive into the network |
| `generator_active_power` | generator `uid` | MW | positive is generation |
| `generator_reactive_power` | generator `uid` | MVAr | positive is generation |
| `generator_voltage_setpoint` | generator `uid` | p.u. | |
| `generator_in_service` | generator `uid` | flag | |
| `load_active_power` | load `uid` | MW | positive is consumption |
| `load_reactive_power` | load `uid` | MVAr | positive is consumption |
| `branch_in_service` | branch `uid` | flag | |
| `branch_tap_ratio` | branch `uid` | ratio | |
| `branch_phase_shift` | branch `uid` | degrees | as `Branch.shift` |
| `switch_closed` | switch `uid` | flag | |

Multiconductor quantities. A terminal quantity is keyed `bus_id/terminal`, a
per phase element quantity `element_name/terminal`, and a whole element
quantity by the element name.

| quantity | keyed by | unit | sign |
|---|---|---|---|
| `terminal_voltage_magnitude` | `bus/terminal` | volts | |
| `terminal_voltage_angle` | `bus/terminal` | radians | |
| `load_active_power` | `load/terminal` | watts | positive is consumption |
| `load_reactive_power` | `load/terminal` | vars | positive is consumption |
| `generator_active_power` | `generator/terminal` | watts | positive is generation |
| `generator_reactive_power` | `generator/terminal` | vars | positive is generation |
| `transformer_tap` | transformer name | ratio | |
| `capacitor_steps` | capacitor name | count | |
| `switch_closed` | switch name | flag | |

## powerio.TimeSeries\<T\>

Ordered time points and one value of `T` per point. Labels are the source's
own; PowerIO imposes no calendar meaning.

Schema definitions: `StoredTimeSeries`, `StoredTimeSeries2`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `time_points` | array of `TimePoint` | | | | required |
| `values` | array of the element type, each a complete network | | | same length as `time_points` | required |

A series of operating points gives the shared base network once.

Schema definitions: `StoredOperatingPointTimeSeries`, `StoredOperatingPointTimeSeries2`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | the base network, or null | | | null only for an empty series | required when `values` is nonempty |
| `time_points` | array of `TimePoint` | | | | required |
| `values` | array of `StoredOperatingPointAssignment` | | | same length as `time_points`; identities name components of `network` | required |

Schema definition: `TimePoint`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `label` | string | | | nonempty, bounded | required |
| `duration` | `Duration` or null | | | the interval the point covers | null |

Schema definition: `Duration`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `secs` | integer | seconds | | | required |
| `nanos` | integer | nanoseconds | | below one billion | required |

## powerio.ScenarioSet\<T\>

Named alternatives of `T` with no implied order. Either every scenario has a
probability or none does; when present, the probabilities are nonnegative and
sum to one within `SCENARIO_PROBABILITY_TOLERANCE`.

Schema definitions: `StoredScenarioSet`, `StoredScenarioSet2`, `StoredScenarioSet3`, `StoredScenarioSet4`, `StoredScenarioSet5`, `StoredScenarioSet6`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `scenarios` | array of the scenario record | | | `id` unique | required |

Schema definitions: `StoredScenario`, `StoredScenario2`, `StoredScenario3`, `StoredScenario4`, `StoredScenario5`, `StoredScenario6`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | string | | | nonempty, bounded, unique in the set | required |
| `probability` | float or null | | | in `[0, 1]` | null |
| `value` | the element type: a network, a network time series, or an operating point time series | | | | required |

A scenario set of operating points gives the shared base network once.

Schema definitions: `StoredOperatingPointScenarioSet`, `StoredOperatingPointScenarioSet2`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | the base network, or null | | | null only for an empty set | required when `scenarios` is nonempty |
| `scenarios` | array of `StoredOperatingPointScenario` | | | `id` unique | required |

Schema definition: `StoredOperatingPointScenario`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | string | | | nonempty, bounded, unique in the set | required |
| `probability` | float or null | | | in `[0, 1]` | null |
| `quantities` | map of quantity name to `StoredQuantity` | | | identities name components of `network` | required |

## Instances

An instance is the complete input to one named calculation, including the
network it runs over. An instance may also include an `initial_point`, a
starting operating assignment a solver may use, whose identities refer to
components of the instance's network.

### powerio.DcPfInstance

Schema definition: `DcPfInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `BalancedNetwork` | | | at least one `REF` bus | required |
| `approximation` | token `series_susceptance`, `tap_adjusted_reactance`, `reactance_only` | | | the DC branch susceptance formula | required |
| `initial_point` | `StoredOperatingPointAssignment` or null | | | | null |

The bus specifications are derived from the network: a `REF` bus contributes
its stated angle, an `ISOLATED` bus no equation, and every other bus its net
active injection over in service generators and loads.

### powerio.AcPfInstance

Schema definition: `AcPfInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `BalancedNetwork` | | | at least one `REF` bus | required |
| `specifications` | array of `AcBusSpecification` | | | one per bus, in bus table order | required |
| `initial_point` | `StoredOperatingPointAssignment` or null | | | | null |

`AcBusSpecification` is tagged by `kind`. `pq` gives `p` (MW) and `q`
(MVAr), the prescribed net injection; `pv` gives `p` and `vm` (p.u.);
`reference` gives `vm` and `va` (degrees); `isolated` gives nothing. Powers
are net injections into the network, positive for generation.

### powerio.DcOpfInstance

Schema definition: `DcOpfInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `BalancedNetwork` | | | at least one `REF` bus and one in service generator on a non isolated bus | required |
| `objective` | `Objective` | | | | required |
| `constraints` | `ActiveConstraints` | | | | required |
| `approximation` | token as `DcPfInstance.approximation` | | | | required |
| `initial_point` | `StoredOperatingPointAssignment` or null | | | | null |

### powerio.AcOpfInstance

Schema definition: `AcOpfInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `BalancedNetwork` | | | at least one `REF` bus and one in service generator on a non isolated bus | required |
| `objective` | `Objective` | | | | required |
| `constraints` | `ActiveConstraints` | | | | required |
| `initial_point` | `StoredOperatingPointAssignment` or null | | | | null |

Schema definition: `Objective`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `terms` | array of `ObjectiveTerm` | | | empty is a feasibility problem | `[]` |

`ObjectiveTerm` is tagged by `term`: `network_generator_cost` sums the
network's generator cost curves; `active_power_dispatch_cost` prices active
dispatch.

Schema definition: `ActiveConstraints`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `generator_capability` | `ConstraintSelection` | | | active and reactive generator limits | required |
| `voltage_bounds` | `ConstraintSelection` | | | bus voltage magnitude bounds | required |
| `thermal_limits` | `ConstraintSelection` | | | branch thermal limits | required |
| `angle_bounds` | `ConstraintSelection` | | | branch angle difference bounds | required |

`ConstraintSelection` is tagged by `select`: `all` (every element with a
stated limit), `none` (the family is relaxed), or `only` with `identities`,
the list of element identities (`uid` values, or bus ids) the family applies
to.

### powerio.McAcPfInstance

Schema definition: `McAcPfInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `MulticonductorNetwork` | | | at least one voltage source | required |
| `initial_point` | `StoredOperatingPointAssignment` or null | | | | null |

The prescribed terminal powers, source voltages, and active regulator and
capacitor controls are derived from the network.

### powerio.McAcOpfInstance

Schema definition: `McAcOpfInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `MulticonductorNetwork` | | | at least one voltage source | required |
| `objective` | `Objective` | | | | required |
| `constraints` | `MulticonductorActiveConstraints` | | | | required |
| `initial_point` | `StoredOperatingPointAssignment` or null | | | | null |

Schema definition: `MulticonductorActiveConstraints`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `terminal_voltage_bounds` | `ConstraintSelection` | | | | required |
| `conductor_limits` | `ConstraintSelection` | | | current or apparent power limits | required |
| `generator_capability` | `ConstraintSelection` | | | per phase bounds | required |

### powerio.AcScucInstance

The DOE GO Challenge 3 formulation: one balanced network plus scheduling,
reserve, and contingency inputs. Powers in the inputs are per unit on the
network's `base_mva`, times are hours from the start of the horizon, and
costs are dollars or dollars per per unit hour, as the data format gives
them.

Schema definition: `AcScucInstance`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `network` | `BalancedNetwork` | | | | required |
| `inputs` | `ScucInputs` | | | identities name components of `network`; time varying records match the horizon | required |

Schema definition: `ScucInputs`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `interval_durations` | array of float | hours | | positive; chronological | required |
| `devices` | array of `ScucDevice` | | | `id` unique | required |
| `active_reserve_zones` | array of `ScucActiveReserveZone` | | | | required |
| `reactive_reserve_zones` | array of `ScucReactiveReserveZone` | | | | required |
| `contingencies` | array of `ScucContingency` | | | | required |
| `shunts` | array of `ScucShunt` | | | | required |
| `transformer_controls` | array of `ScucTransformerControl` | | | | required |
| `branch_switching_costs` | array of `ScucBranchSwitchingCost` | | | | required |
| `violation_costs` | `ScucViolationCosts` | | | | required |

Schema definition: `ScucDevice`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | the generator or load in the network | required |
| `kind` | token `producer`, `consumer` | | | | required |
| `on_cost` | float | dollars | | | required |
| `startup_cost` | float | dollars | | | required |
| `shutdown_cost` | float | dollars | | | required |
| `initial_on_status` | boolean | | | | required |
| `initial_commitment` | `ScucInitialCommitment` | | | | required |
| `minimum_up_time` | float | hours | | | required |
| `minimum_down_time` | float | hours | | | required |
| `ramp_limits` | `ScucRampLimits` | | | | required |
| `reserve_limits` | `ScucReserveLimits` | | | | required |
| `reactive_capability` | `ScucReactiveCapability` | | | | required |
| `energy_lower_bounds` | array of `ScucEnergyRequirement` | | | | required |
| `energy_upper_bounds` | array of `ScucEnergyRequirement` | | | | required |
| `startup_cost_adjustments` | array of `ScucStartupCostAdjustment` | | | | required |
| `startup_limits` | array of `ScucStartupLimit` | | | | required |
| `periods` | array of `ScucDevicePeriod` | | | one per interval, chronological | required |

`ScucReactiveCapability` is tagged by `kind`: `none`; `linear` with
`reactive_power_at_zero_active_power` and a slope; `bounded` with the
`_max` and `_min` forms of both.

Schema definition: `ScucDevicePeriod`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `on_status_min` | boolean | | | | required |
| `on_status_max` | boolean | | | | required |
| `active_power_min` | float | p.u. | | `active_power_min <= active_power_max` | required |
| `active_power_max` | float | p.u. | | | required |
| `reactive_power_min` | float | p.u. | | `reactive_power_min <= reactive_power_max` | required |
| `reactive_power_max` | float | p.u. | | | required |
| `energy_cost_blocks` | array of `ScucEnergyCostBlock` | | | | required |
| `reserve_costs` | `ScucReserveCosts` | | | | required |

Schema definition: `ScucEnergyCostBlock`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `block_size` | float | p.u. | | nonnegative | required |
| `marginal_cost` | float | dollars per p.u. hour | | | required |

Schema definition: `ScucReserveCosts`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `regulation_up` | float | dollars per p.u. hour | | | required |
| `regulation_down` | float | dollars per p.u. hour | | | required |
| `synchronized` | float | dollars per p.u. hour | | | required |
| `nonsynchronized` | float | dollars per p.u. hour | | | required |
| `ramping_up_online` | float | dollars per p.u. hour | | | required |
| `ramping_up_offline` | float | dollars per p.u. hour | | | required |
| `ramping_down_online` | float | dollars per p.u. hour | | | required |
| `ramping_down_offline` | float | dollars per p.u. hour | | | required |
| `reactive_up` | float | dollars per p.u. hour | | | required |
| `reactive_down` | float | dollars per p.u. hour | | | required |

Schema definition: `ScucReserveLimits`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `regulation_up` | float | p.u. | | nonnegative | required |
| `regulation_down` | float | p.u. | | nonnegative | required |
| `synchronized` | float | p.u. | | nonnegative | required |
| `nonsynchronized` | float | p.u. | | nonnegative | required |
| `ramping_up_online` | float | p.u. | | nonnegative | required |
| `ramping_up_offline` | float | p.u. | | nonnegative | required |
| `ramping_down_online` | float | p.u. | | nonnegative | required |
| `ramping_down_offline` | float | p.u. | | nonnegative | required |

Schema definition: `ScucRampLimits`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `up` | float | p.u. per hour | | nonnegative | required |
| `down` | float | p.u. per hour | | nonnegative | required |
| `startup` | float | p.u. per hour | | nonnegative | required |
| `shutdown` | float | p.u. per hour | | nonnegative | required |

Schema definition: `ScucInitialCommitment`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `accumulated_up_time` | float | hours | | nonnegative | required |
| `accumulated_down_time` | float | hours | | nonnegative | required |

Schema definition: `ScucEnergyRequirement`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `start_time` | float | hours | | `start_time <= end_time` | required |
| `end_time` | float | hours | | | required |
| `energy` | float | p.u. hour | | | required |

Schema definition: `ScucStartupCostAdjustment`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `maximum_down_time` | float | hours | | | required |
| `cost` | float | dollars | | | required |

Schema definition: `ScucStartupLimit`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `start_time` | float | hours | | `start_time <= end_time` | required |
| `end_time` | float | hours | | | required |
| `maximum_startups` | integer | | | | required |

Schema definition: `ScucActiveReserveZone`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | unique | required |
| `buses` | array of `ComponentId` | | | each names a bus of the network | required |
| `regulation_up_requirement_fraction` | float | fraction of zone load | | | required |
| `regulation_down_requirement_fraction` | float | fraction of zone load | | | required |
| `synchronized_requirement_fraction` | float | fraction of zone load | | | required |
| `nonsynchronized_requirement_fraction` | float | fraction of zone load | | | required |
| `ramping_up_requirement` | array of float | p.u. | | one per interval | required |
| `ramping_down_requirement` | array of float | p.u. | | one per interval | required |
| `regulation_up_violation_cost` | float | dollars per p.u. hour | | | required |
| `regulation_down_violation_cost` | float | dollars per p.u. hour | | | required |
| `synchronized_violation_cost` | float | dollars per p.u. hour | | | required |
| `nonsynchronized_violation_cost` | float | dollars per p.u. hour | | | required |
| `ramping_up_violation_cost` | float | dollars per p.u. hour | | | required |
| `ramping_down_violation_cost` | float | dollars per p.u. hour | | | required |

Schema definition: `ScucReactiveReserveZone`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | unique | required |
| `buses` | array of `ComponentId` | | | each names a bus of the network | required |
| `reactive_up_requirement` | array of float | p.u. | | one per interval | required |
| `reactive_down_requirement` | array of float | p.u. | | one per interval | required |
| `reactive_up_violation_cost` | float | dollars per p.u. hour | | | required |
| `reactive_down_violation_cost` | float | dollars per p.u. hour | | | required |

Schema definition: `ScucContingency`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | unique | required |
| `components` | array of `ComponentId` | | | Challenge 3 requires exactly one AC line, transformer, or DC line | required |

Schema definition: `ScucShunt`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | the shunt in the network | required |
| `initial_step` | integer | | | `step_min <= initial_step <= step_max` | required |
| `step_min` | integer | | | | required |
| `step_max` | integer | | | | required |
| `conductance_per_step` | float | p.u. | | | required |
| `susceptance_per_step` | float | p.u. | positive is capacitive | | required |

Schema definition: `ScucTransformerControl`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | the two winding transformer in the network | required |
| `tap_ratio_min` | float | p.u. | | `tap_ratio_min <= tap_ratio_max` | required |
| `tap_ratio_max` | float | p.u. | | | required |
| `phase_shift_min` | float | radians | | `phase_shift_min <= phase_shift_max` | required |
| `phase_shift_max` | float | radians | | | required |

Schema definition: `ScucBranchSwitchingCost`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | `ComponentId` | | | the branch or transformer in the network | required |
| `connection_cost` | float | dollars | | | required |
| `disconnection_cost` | float | dollars | | | required |

Schema definition: `ScucViolationCosts`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `active_power_balance` | float | dollars per p.u. hour | | | required |
| `reactive_power_balance` | float | dollars per p.u. hour | | | required |
| `branch_thermal_limit` | float | dollars per p.u. hour | | | required |
| `energy_requirement` | float | dollars per p.u. hour | | | required |

## Solutions

A solution contains the instance it solves, and its arrays follow the
instance network's table order: one entry per bus, per branch, or per
generator. Injections are net injections into the network at a bus,
positive for generation; branch flows are measured into the branch at the
named terminal. Each solution says how the calculation ended and what
residuals the producer reported. `producer` is the producer's free text
solver identity, or null.

`Termination` is tagged by `kind`: `converged`, `iteration_limit`,
`infeasible`, `unbounded`, `failed`, or `not_reported`, for a source that
stores a solved calculation without termination information, as DeepMind
OPFData does.

Schema definition: `Residuals`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `max_active_power_mismatch` | float or null | MW | | largest absolute active balance mismatch | null (not reported) |
| `max_reactive_power_mismatch` | float or null | MVAr | | largest absolute reactive balance mismatch | null (not reported) |

Schema definition: `GeneratorDispatch`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `p_mw` | array of float | MW | positive is generation | one per generator | required |
| `q_mvar` | array of float | MVAr | positive is generation | one per generator, or empty | required |

Schema definition: `ThreeWindingTransformerTerminalActivePower`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `p_mw` | array of float | MW | positive flows into the transformer at the winding | three entries, winding order | required |

Schema definition: `ThreeWindingTransformerTerminalPower`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `p_mw` | array of float | MW | positive flows into the transformer at the winding | three entries, winding order | required |
| `q_mvar` | array of float | MVAr | positive flows into the transformer at the winding | three entries, winding order | required |

### powerio.DcPfSolution

Schema definition: `DcPfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `DcPfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `bus_voltage_angle` | array of float | degrees | | one per bus | required |
| `bus_active_injection` | array of float | MW | positive into the network | one per bus | required |
| `branch_from_active_flow` | array of float | MW | positive into the branch at the from terminal | one per branch | required |
| `branch_to_active_flow` | array of float | MW | positive into the branch at the to terminal | one per branch | required |
| `three_winding_transformer_terminal_active_powers` | array of `ThreeWindingTransformerTerminalActivePower` | | | one per three winding transformer | required |
| `generator_dispatch` | `GeneratorDispatch` or null | | | | null |

### powerio.AcPfSolution

Schema definition: `AcPfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `AcPfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `bus_voltage_magnitude` | array of float | p.u. | | one per bus | required |
| `bus_voltage_angle` | array of float | degrees | | one per bus | required |
| `bus_active_injection` | array of float | MW | positive into the network | one per bus | required |
| `bus_reactive_injection` | array of float | MVAr | positive into the network | one per bus | required |
| `branch_from_active_flow` | array of float | MW | positive into the branch at the from terminal | one per branch | required |
| `branch_from_reactive_flow` | array of float | MVAr | positive into the branch at the from terminal | one per branch | required |
| `branch_to_active_flow` | array of float | MW | positive into the branch at the to terminal | one per branch | required |
| `branch_to_reactive_flow` | array of float | MVAr | positive into the branch at the to terminal | one per branch | required |
| `three_winding_transformer_terminal_powers` | array of `ThreeWindingTransformerTerminalPower` | | | one per three winding transformer | required |
| `generator_dispatch` | `GeneratorDispatch` or null | | | | null |

### powerio.DcOpfSolution

Multipliers and marginals are the optimizing producer's optional economic
outputs, in the objective's units per MW; no currency is assumed.

Schema definition: `DcOpfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `DcOpfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `bus_voltage_angle` | array of float | degrees | | one per bus | required |
| `bus_active_injection` | array of float | MW | positive into the network | one per bus | required |
| `branch_from_active_flow` | array of float | MW | positive into the branch at the from terminal | one per branch | required |
| `branch_to_active_flow` | array of float | MW | positive into the branch at the to terminal | one per branch | required |
| `generator_active_power` | array of float | MW | positive is generation | one per generator | required |
| `three_winding_transformer_terminal_active_powers` | array of `ThreeWindingTransformerTerminalActivePower` | | | one per three winding transformer | required |
| `objective` | float | objective units | | | required |
| `bus_active_power_marginal` | array of float or null | objective units per MW | | one per bus | null |
| `branch_from_limit_multiplier` | array of float or null | objective units per MW | | one per branch; finite and nonnegative | null |
| `branch_to_limit_multiplier` | array of float or null | objective units per MW | | one per branch; finite and nonnegative | null |

### powerio.AcOpfSolution

Schema definition: `AcOpfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `AcOpfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `bus_voltage_magnitude` | array of float | p.u. | | one per bus | required |
| `bus_voltage_angle` | array of float | degrees | | one per bus | required |
| `bus_active_injection` | array of float | MW | positive into the network | one per bus | required |
| `bus_reactive_injection` | array of float | MVAr | positive into the network | one per bus | required |
| `branch_from_active_flow` | array of float | MW | positive into the branch at the from terminal | one per branch | required |
| `branch_from_reactive_flow` | array of float | MVAr | positive into the branch at the from terminal | one per branch | required |
| `branch_to_active_flow` | array of float | MW | positive into the branch at the to terminal | one per branch | required |
| `branch_to_reactive_flow` | array of float | MVAr | positive into the branch at the to terminal | one per branch | required |
| `generator_active_power` | array of float | MW | positive is generation | one per generator | required |
| `generator_reactive_power` | array of float | MVAr | positive is generation | one per generator | required |
| `three_winding_transformer_terminal_powers` | array of `ThreeWindingTransformerTerminalPower` | | | one per three winding transformer | required |
| `objective` | float | objective units | | | required |
| `bus_active_power_marginal` | array of float or null | objective units per MW | | one per bus | null |
| `bus_reactive_power_marginal` | array of float or null | objective units per MVAr | | one per bus | null |
| `branch_from_limit_multiplier` | array of float or null | objective units per MVA | | one per branch; finite and nonnegative | null |
| `branch_to_limit_multiplier` | array of float or null | objective units per MVA | | one per branch; finite and nonnegative | null |

### powerio.SocwrOpfSolution

The PowerModels SOCWR relaxation of an `AcOpfInstance`. Its objective is a
lower bound, and its voltage products make no claim of an AC feasible
phasor.

Schema definition: `SocwrOpfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `AcOpfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `values` | `SocwrOpfValues` | | | | required |
| `duals` | `SocwrOpfDuals` | | | | every member null |
| `objective_lower_bound` | float | objective units | | | required |

Schema definition: `SocwrOpfValues`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus_voltage_magnitude_squared` | array of float | p.u. squared | | `w[i] = \|V_i\|^2`; one per bus | required |
| `branch_voltage_product_real` | array of float | p.u. squared | | `Re(V_from conj(V_to))`; one per branch | required |
| `branch_voltage_product_imaginary` | array of float | p.u. squared | | `Im(V_from conj(V_to))`; one per branch | required |
| `generator_active_power` | array of float | MW | positive is generation | one per generator | required |
| `generator_reactive_power` | array of float | MVAr | positive is generation | one per generator | required |
| `branch_from_active_power` | array of float | MW | positive into the branch at the from terminal | one per branch | required |
| `branch_from_reactive_power` | array of float | MVAr | positive into the branch at the from terminal | one per branch | required |
| `branch_to_active_power` | array of float | MW | positive into the branch at the to terminal | one per branch | required |
| `branch_to_reactive_power` | array of float | MVAr | positive into the branch at the to terminal | one per branch | required |
| `three_winding_transformer_terminal_powers` | array of `ThreeWindingTransformerTerminalPower` | | | one per three winding transformer | required |

Schema definition: `SocwrOpfDuals`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus_active_power_marginal` | array of float or null | objective units per MW | | one per bus | null |
| `bus_reactive_power_marginal` | array of float or null | objective units per MVAr | | one per bus | null |
| `branch_from_thermal_limit_multiplier` | array of float or null | objective units per MVA | | one per branch; finite and nonnegative | null |
| `branch_to_thermal_limit_multiplier` | array of float or null | objective units per MVA | | one per branch; finite and nonnegative | null |

### powerio.McAcPfSolution

Terminal columns run in bus table order and, within a bus, in the bus's
stated terminal order. Source columns run in source table order and, within
a source, in its `terminal_map` order.

Schema definition: `McAcPfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `McAcPfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `terminal_voltage_magnitude` | array of float | volts | | one per terminal | required |
| `terminal_voltage_angle` | array of float | radians | | one per terminal | required |
| `terminal_current_magnitude` | array of float or null | amperes | | one per terminal | null |
| `terminal_active_power` | array of float or null | watts | positive into the network | one per terminal | null |
| `source_active_injection` | array of float | watts | positive into the network | one per source terminal | required |

### powerio.McAcOpfSolution

Schema definition: `McAcOpfSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `McAcOpfInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `terminal_voltage_magnitude` | array of float | volts | | one per terminal | required |
| `terminal_voltage_angle` | array of float | radians | | one per terminal | required |
| `terminal_current_magnitude` | array of float or null | amperes | | one per terminal | null |
| `terminal_active_power` | array of float or null | watts | positive into the network | one per terminal | null |
| `source_active_injection` | array of float | watts | positive into the network | one per source terminal | required |
| `generator_active_power` | array of float | watts | positive is generation | generator table order, each generator's `terminal_map` order | required |
| `objective` | float | objective units | | | required |

### powerio.AcScucSolution

The GO Challenge 3 output fields. Every series is `values[t][row]` over the
instance's time points and the corresponding table order; a series a
producer did not supply is empty.

Schema definition: `AcScucSolution`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `instance` | `AcScucInstance` | | | | required |
| `termination` | `Termination` | | | | required |
| `residuals` | `Residuals` | | | | required |
| `producer` | string or null | | | | null |
| `network_outputs` | `ScucNetworkOutputs` | | | | required |
| `device_outputs` | `ScucDeviceOutputs` | | | | required |
| `objective` | float or null | dollars | | | null |

Schema definition: `ScucNetworkOutputs`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `bus_vm` | array of array of float | p.u. | | one row per time point, one per bus | required |
| `bus_va` | array of array of float | radians | | one per bus | required |
| `shunt_step` | array of array of integer | | | one per shunt | required |
| `ac_line_on_status` | array of array of boolean | | | one per AC line | required |
| `transformer_tm` | array of array of float | ratio | | one per two winding transformer | required |
| `transformer_ta` | array of array of float | radians | | one per two winding transformer | required |
| `transformer_on_status` | array of array of boolean | | | one per two winding transformer | required |
| `dc_line_pdc_fr` | array of array of float | p.u. | positive from the from bus to the to bus | one per DC line | `[]` |
| `dc_line_qdc_fr` | array of array of float | p.u. | positive is injected into the from bus | one per DC line | `[]` |
| `dc_line_qdc_to` | array of array of float | p.u. | positive is injected into the to bus | one per DC line | `[]` |

Schema definition: `ScucDeviceOutputs`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `on_status` | array of array of boolean | | | one row per time point, one per device | required |
| `startup_status` | array of array of boolean | | | | required |
| `shutdown_status` | array of array of boolean | | | | required |
| `p_on` | array of array of float | p.u. | positive is production for a producer, consumption for a consumer | | required |
| `q` | array of array of float | p.u. | as `p_on` | | required |
| `p_reg_res_up` | array of array of float | p.u. | | nonnegative | required |
| `p_reg_res_down` | array of array of float | p.u. | | nonnegative | required |
| `p_syn_res` | array of array of float | p.u. | | nonnegative | required |
| `p_nsyn_res` | array of array of float | p.u. | | nonnegative | required |
| `p_ramp_res_up_online` | array of array of float | p.u. | | nonnegative | required |
| `p_ramp_res_up_offline` | array of array of float | p.u. | | nonnegative | required |
| `p_ramp_res_down_online` | array of array of float | p.u. | | nonnegative | required |
| `p_ramp_res_down_offline` | array of array of float | p.u. | | nonnegative | required |
| `q_res_up` | array of array of float | p.u. | | nonnegative | required |
| `q_res_down` | array of array of float | p.u. | | nonnegative | required |

## Module records

The records beside `value` in a `.pio.json` document.
[.pio.json schema](pio-json-schema.md) explains what each one is for; the
tables here list their fields.

Schema definition: `Producer`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `name` | string | | | nonempty, bounded | required |
| `version` | string | | | nonempty, bounded | required |

Schema definition: `SourceDescriptor`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | string | | | unique among `sources`; spans and source map entries name it | required |
| `name` | string | | | a file name, never a local file path | required |
| `byte_length` | integer | bytes | | every span into this source ends at or before it | required |
| `format` | string or null | | | a format token | null |
| `digest` | `Digest` or null | | | | null |

Schema definition: `Digest`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `algorithm` | token `sha256` | | | | required |
| `value` | string | | | 64 lowercase hexadecimal characters | required |

Schema definition: `SourceSpan`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `source` | string | | | names a source `id` | required |
| `byte_start` | integer | bytes into the retained source | | `byte_start <= byte_end <= byte_length` | required |
| `byte_end` | integer | bytes into the retained source | | half open | required |

Schema definition: `SourceMapEntry`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `target` | string | | | an RFC 6901 pointer into `value.data` | required |
| `relation` | token `exact`, `defaulted`, `inferred`, `converted_units`, `aggregated`, `split`, `synthetic`, `transformed`, `retained_extra` | | | | required |
| `spans` | array of `SourceSpan` | | | empty only for `defaulted`, `synthetic`, and `transformed` | `[]` |

Schema definition: `Diagnostic`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | string | | | unique among `diagnostics`; assigned `d0`, `d1`, ... at serialization when a record has none | required |
| `code` | string | | | `NAMESPACE.SCOPE.SPECIFIC` | required |
| `severity` | token `error`, `warning`, `remark`, `note` | | | | required |
| `message` | string | | | one line | required |
| `target` | string or null | | | an RFC 6901 pointer into `value.data` | null |
| `spans` | array of `SourceSpan` | | | the records the finding is about | `[]` |
| `related` | array of string | | | each names a diagnostic `id` in the document | `[]` |
| `details` | object | | | | `{}` |
| `suggested_action` | string or null | | | | null |

Schema definition: `HistoryEntry`.

| field | type | unit | sign | invariant | if absent |
|---|---|---|---|---|---|
| `id` | string | | | unique among `history` | required |
| `kind` | token `parse`, `transform`, `edit`, `repair`, `solve` | | | | required |
| `name` | string | | | the operation | required |
| `input_type` | string or null | | | a structural type name | null |
| `output_type` | string or null | | | a structural type name | null |
| `parameters` | object | | | | `{}` |
| `assumptions` | array of string | | | | `[]` |
| `losses` | array of string | | | | `[]` |
