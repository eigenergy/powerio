# Rejection audit

Every `Error` severity diagnostic PowerIO raises while parsing a source,
building an instance, or preparing numerical arrays, and why it is or is not a
refusal.

A rejection belongs in one of two classes.

- **(a) Unusable data.** The values do not state a thing the operation can act
  on: a shape that does not match, a value that is not a number, an identity
  the model does not declare. No option makes them usable, so the operation
  ends and names what is wrong.
- **(b) A consumer's judgement.** The data states something a particular
  consumer will not accept, while another consumer reads the same case without
  ever touching it. Convexity of a cost curve, a zero impedance branch, a
  missing angle reference, an empty dispatch set. PowerIO does not hold one
  consumer's requirement over the rest: a class (b) code is a warning under a
  documented default, or the caller opts into the refusal.

A class (b) rejection can still be the default when the alternative would be
for PowerIO to invent data. Where that is the case, the default refusal points
at the explicit, checked transformation that resolves it, and the row below
names that transformation.

## Options and their defaults

Every strictness knob is a field on an options struct with a `Default` impl,
and carries the same name in Rust, the C ABI, Python, the MCP tools, and the
CLI.

| Option | Default | Rust | C ABI | Python | CLI |
| --- | --- | --- | --- | --- | --- |
| `skip_zero_impedance` | `false` | `BuildOptions`, `DcOperatorOptions`, `DcOpfAssemblyOptions`, `AcOpfAssemblyOptions`, `AcPfAssemblyOptions` | `PioOpfBuildOptions`, `pio_build_*_preparation` | `skip_zero_impedance=False` | `--skip-zero-impedance` |
| `synthesize_unrated_limits` | `false` | `DcOpfAssemblyOptions`, `AcOpfAssemblyOptions` | `PioOpfBuildOptions`, `pio_build_*_preparation` | not exposed | `--synthesize-unrated-limits` |
| `correct_angle_difference_bounds` | `true` | `DcOpfAssemblyOptions`, `AcOpfAssemblyOptions`, `AcPfAssemblyOptions` | `PioOpfBuildOptions`, `pio_build_*_preparation` | not exposed | not exposed |
| `cost_curve_policy` | `any` | `DcOpfAssemblyOptions`, `AcOpfAssemblyOptions` | `PioOpfBuildOptions` | `cost_curve_projections(policy=...)` | `--cost-curve-policy` |
| `unsupported` | `reject` | `LinDist3FlowBuildOptions` | not exposed | not exposed | not exposed |
| `reference_policy` | `auto` | `LinDist3FlowBuildOptions` | not exposed | not exposed | not exposed |
| `require_neutral_provenance` | `false` | `LinDist3FlowBuildOptions` | not exposed | not exposed | not exposed |

An option is absent from a column only where that entry point has no operation
the option applies to. `correct_angle_difference_bounds` and the LinDist3Flow
policies have no binding surface yet; those are gaps, not deliberate
divergences.

## `BUILD.BRANCH.*` and `BUILD.INDEX.*` (powerio-tx)

| Code | Raised when | Class | Decision |
| --- | --- | --- | --- |
| `BUILD.BRANCH.ZERO_IMPEDANCE` | the selected matrix denominator is zero for a branch | (b) | `skip_zero_impedance`, default off. Skipping deletes an edge from the topology the result describes, so the lossless resolution is `merge_zero_impedance_buses`, which merges the buses and reports each merge. |
| `BUILD.BRANCH.NOT_A_NUMBER` | a branch's `r`, `x`, or `b` gives a non-finite susceptance | (a) | Stays. No matrix entry exists to carry. |
| `BUILD.BRANCH.DEGENERATE_TAP` | a tap ratio is too small to divide by | (a) | Stays. `tap == 0` already reads as `tap = 1`; a tap below that is a value no formula can use. |
| `BUILD.INDEX.UNKNOWN_BUS` | an element names a bus id the bus table does not declare | (a) | Stays. The element is attached to nothing. |
| `BUILD.INDEX.REFERENCE_BUS_COUNT` | `ReferenceBuses::single()` is called on a set that does not hold exactly one | (a) | Stays. It is a query precondition, not a build: `iter()` reads any set. |
| `BUILD.INDEX.UNGROUNDED_COMPONENT` | a connected component states no reference bus | (b) | Default refusal. The grounded Laplacian of an ungrounded island is singular, so the prepared arrays would describe no solvable system. `to_normalized` designates a reference and reports `CANONICALIZE.NORMALIZE.REFERENCE_DESIGNATED`; that is the explicit resolution. |
| `BUILD.GEO.UNLOCATED_ELEMENTS` | a geo apply left a bus with no location or a branch with no route | (b) | Already resolved: applying a layer returns a `GeoApplyReport` and refuses nothing. `GeoApplyReport::require_located()` is the opt-in refusal. |

## `BUILD.INSTANCE.*` and `BUILD.OPERATING_POINT.*` (powerio-prob)

| Code | Raised when | Class | Decision |
| --- | --- | --- | --- |
| `BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX` | a piecewise linear cost row's segment slopes decrease | (b) | `cost_curve_policy`, default `any`: the curve reaches the arrays as stated and the code is emitted at `Warning`. `convexify_lower_envelope` replaces it; `convex_only` refuses. |
| `BUILD.INSTANCE.CONCAVE_COST` | a polynomial cost row has a negative quadratic coefficient | (b) | As above. |
| `BUILD.INSTANCE.PIECEWISE_COST_INVALID` | a piecewise row is truncated, has fewer than two breakpoints, or has non-increasing or non-finite power | (a) | Stays. The row does not state a curve to carry. |
| `BUILD.INSTANCE.UNSUPPORTED_COST_MODEL` | the model number is not 1 or 2, or a model 2 row is cubic or higher after leading rounding artifacts come off | (a) | Stays. An unknown model number states no curve, and truncating a higher degree polynomial to quadratic would change the price the source states. A consumer that wants the truncation edits the row. |
| `BUILD.INSTANCE.NO_GENERATORS` | an OPF instance has no in service generator on a non-isolated bus | (b) | Default refusal. An OPF with an empty dispatch set is a power flow, and `DcPfInstance`/`AcPfInstance` is the entry point that states it; building it as an OPF would give a problem with no decision variables. |
| `BUILD.INSTANCE.NO_REFERENCE_BUS` | a network declares no `BusType::Ref` | (b) | Default refusal, as `BUILD.INDEX.UNGROUNDED_COMPONENT`, with the same explicit resolution in `to_normalized`. |
| `BUILD.INSTANCE.VOLTAGE_CONTROL_CONFLICT` | two in service generators at one bus state different voltage setpoints | (a) | Stays. The bus specification would have to pick one setpoint over the other, which is an edit the data does not state. |
| `BUILD.INSTANCE.SHAPE_MISMATCH` | a calculation input's row count differs from the network's element table | (a) | Stays. |
| `BUILD.OPERATING_POINT.SHAPE_MISMATCH` | an operating point column disagrees with the resolved identity layout | (a) | Stays. |
| `BUILD.OPERATING_POINT.IDENTITY_UNKNOWN` | an operating point names an element the network does not declare | (a) | Stays. |
| `BUILD.SOLUTION.SHAPE_MISMATCH` | a solution column disagrees with the instance's element tables | (a) | Stays. |
| `BUILD.SOLUTION.MULTIPLIER_INVALID` | a constraint multiplier is negative or non-finite | (a) | Stays. A sign convention violation makes the multiplier unreadable, not merely unusual. |
| `BUILD.OPERATOR.ZERO_IMPEDANCE` | a zero impedance branch has no finite DC operator row | (b) | `skip_zero_impedance` on `DcOperatorOptions`, as `BUILD.BRANCH.ZERO_IMPEDANCE`. |
| `BUILD.OPERATOR.NOT_A_NUMBER` | a branch value produced a non-finite operator entry | (a) | Stays. |

## `BUILD.LINDIST3FLOW.*` (powerio-prob)

The LinDist3Flow builder already has the shape the rest of this table aims at:
`LinDist3FlowBuildOptions` carries `unsupported` (`reject`, `lower`,
`approximate`, `permissive`), `reference_policy`, and
`require_neutral_provenance`.

| Code | Class | Decision |
| --- | --- | --- |
| `BUILD.LINDIST3FLOW.UNSUPPORTED_COMPONENT` | (b) | `unsupported`, default `reject`. `permissive` carries the case without the component. |
| `BUILD.LINDIST3FLOW.EXPLICIT_NEUTRAL` | (b) | `require_neutral_provenance`, default off; the refusal stands only for a network that still carries an explicit neutral, which the affine model has no equation for. |
| `BUILD.LINDIST3FLOW.REFERENCE_INVALID` | (b) | `reference_policy`, default `auto`. |
| `BUILD.LINDIST3FLOW.POLICY_UNAVAILABLE` | (a) | Stays. The caller named a policy this build does not implement; the request, not the data, is the fault. |
| `BUILD.LINDIST3FLOW.TOPOLOGY_INVALID` | (a) | Stays. The model is defined on a source-rooted forest; a cycle has no LinDist3Flow equation. |
| `BUILD.LINDIST3FLOW.OBJECTIVE_UNSUPPORTED` | (a) | Stays, as `POLICY_UNAVAILABLE`. |
| `BUILD.LINDIST3FLOW.DEVICE_INVALID` | (a) | Stays. |
| `BUILD.LINDIST3FLOW.COEFFICIENT_INVALID` (`BUILD.MULTI.*`) | (a) | Stays. |

## `BUILD.OPF.*`, `BUILD.MATRIX.*`, `BUILD.SENSITIVITY.*`, `BUILD.GRIDFM.*` (powerio-matrix)

| Code | Raised when | Class | Decision |
| --- | --- | --- | --- |
| `BUILD.OPF.NODAL_COST_UNSUPPORTED` | a bus space quadratic cannot carry a piecewise or concave generator column | (b) | `calc_nodal_generator_data` still refuses: the parallel rule describes the least cost split only over convex curves, and returning a number that does not is worse than returning none. `emit_dcopf_bundle` no longer refuses the whole bundle over it; it writes every other file, leaves `q.mtx`, `c.mtx`, and `c0.mtx` out, and reports this code at `Warning`. |
| `BUILD.OPF.OBJECTIVE_UNSUPPORTED` | the instance objective has terms the balanced preparation does not compile | (a) | Stays. The request names a formulation this build does not have. |
| `BUILD.OPF.CONSTRAINT_IDENTITY_UNKNOWN` | an active constraint selection names no element in its family | (a) | Stays. A silently dropped selection would leave a constraint the caller asked for out of the problem. |
| `BUILD.OPF.ELEMENT_IDENTITY_DUPLICATE` | an element family does not have unique stable identities | (a) | Stays. Selections address elements by identity, so a duplicate makes every selection ambiguous. |
| `BUILD.AC_PF.SPECIFICATION_UNSUPPORTED` | a bus specification variant the AC power flow preparation does not implement | (a) | Stays. |
| `BUILD.MATRIX.SHAPE_MISMATCH` | an operand's length does not match the matrix it is used with | (a) | Stays. |
| `BUILD.SENSITIVITY.SINGULAR` | the reference grounded Laplacian is singular although every component is grounded | (a) | Stays. There is no factorization to return. |
| `BUILD.SENSITIVITY.INVALID_OPTION` | a sensitivity option is outside its domain | (a) | Stays, as `POLICY_UNAVAILABLE`. |
| `BUILD.GRIDFM.EMPTY_BATCH` | a scenario batch holds no snapshot | (a) | Stays. |
| `BUILD.GRIDFM.SCENARIO_ID_OVERFLOW` | numbering a snapshot overflows the scenario id | (a) | Stays. |
| `BUILD.GRIDFM.NORMALIZED_SNAPSHOT` | a snapshot is normalized and the export states raw units | (a) | Stays. The values would be written under the wrong unit. |
| `BUILD.GRIDFM.NOT_A_NUMBER` | a snapshot field is not finite | (a) | Stays. |
| `BUILD.GRIDFM.SCENARIO_SHAPE_MISMATCH` | a snapshot does not share the batch's base element set | (a) | Stays. |
| `BUILD.MULTI.PHYSICS_UNSUPPORTED` | the requested multiconductor calculation needs equipment equations this build has none of | (a) | Stays. `BUILD.MULTI.UNSUPPORTED_STAMP` is already the `Warning` for the omit-and-continue case. |
| `BUILD.DIST.ELECTRICAL_INCOMPLETE` (powerio-dist) | a distribution network states no complete electrical model | (a) | Stays. |

## What changed

Class (b) rows that were refusals and are now warnings or opt-ins:

- `BUILD.INSTANCE.PIECEWISE_COST_NONCONVEX` and `BUILD.INSTANCE.CONCAVE_COST`
  became `cost_curve_policy`, default `any`.
- `BUILD.OPF.NODAL_COST_UNSUPPORTED` no longer loses a whole DC OPF bundle.
- `powerio dcopf` gained `--skip-zero-impedance` and
  `--synthesize-unrated-limits`, which every other entry point already had.

Class (b) rows that stay refusals, each pointing at the explicit
transformation that resolves them: `BUILD.BRANCH.ZERO_IMPEDANCE` and
`BUILD.OPERATOR.ZERO_IMPEDANCE` (`merge_zero_impedance_buses`),
`BUILD.INDEX.UNGROUNDED_COMPONENT` and `BUILD.INSTANCE.NO_REFERENCE_BUS`
(`to_normalized`), `BUILD.INSTANCE.NO_GENERATORS` (`DcPfInstance`).
