# Migrating from 0.10

PowerIO 1.0 corrects OPF preparation and solution contracts found during the
0.10 consumer integration. The `.pio.json` schema remains `powerio.module/1`,
but the stored OPF result fields and the Rust, Python, and Julia APIs changed.
C ABI 6 remains compatible.

## OPF preparation follows the instance

`build_dc_opf_preparation` and `build_ac_opf_preparation` compile the objective
and active constraints declared on the instance. The supported balanced
objectives are an empty objective, which produces a feasibility problem with
zero cost coefficients, and one `network_generator_cost` term. Other terms
return `BUILD.OPF.OBJECTIVE_UNSUPPORTED` instead of silently using network
costs.

Generator space preparation preserves a convex MATPOWER model 1 cost in
`generators.piecewise_linear`, aligned with the generator identities and dense
columns. Its breakpoint powers use the preparation power unit; objective
values are unchanged. The polynomial `q`, `c`, and `c0` entries for that
generator are zero. Malformed and nonconvex curves return typed errors instead
of being fitted or silently convexified.

`calc_nodal_generator_data` returns `Result`. It rejects a piecewise curve
because one nodal quadratic cannot represent it. `emit_dcopf_bundle` uses that
projection and returns the same error; use the generator space preparation for
an exact piecewise objective. The released `nodal_generator_data` and
`write_dcopf_bundle` names were removed in 1.0.

Each preparation carries stable identities, analysis rows, and source rows
beside its dense bus, generator, and branch arrays. Source rows use
`Vec<Option<usize>>`: an original row is `Some(row)`, while a bus or branch
created by three winding transformer lowering is `None`. Synthetic winding
identities use `{transformer identity}/winding:{1|2|3}`. A bus explicitly
typed isolated states no OPF equation; it and every incident element are
absent from the dense arrays, while the source module remains unchanged.
Active constraint masks use the same dense order. An unknown identity in
`ConstraintSelection::Only` returns
`BUILD.OPF.CONSTRAINT_IDENTITY_UNKNOWN`.

## Economic result fields state derivatives

The 0.10 price fields and signed branch dual are replaced in 1.0.
Use these builders and accessors:

- `with_bus_active_power_marginals` records the optimal objective derivative
  per added MW of demand.
- AC `with_bus_reactive_power_marginals` records the derivative per added MVAr.
- `with_branch_thermal_limit_multipliers(from, to)` records the two
  nonnegative thermal constraint multipliers separately.

The bus value has objective units per selected power unit. Call it an LMP only
when the instance objective supports that interpretation. For a shared branch
rating, the local derivative of the optimal objective is the negative sum of
the from and to multipliers.

Stored solution fields use
`bus_active_power_marginal`, `bus_reactive_power_marginal`,
`branch_from_limit_multiplier`, and `branch_to_limit_multiplier`.

The 0.10 names still read. `bus_price`, `bus_active_price`, and
`bus_reactive_price` were documented as locational marginal prices: the
optimal objective change per added demand under the same positive sign, so
they map directly to the corresponding demand marginal. The signed DC
`branch_flow_dual` used `from - to`; 1.0 maps a positive value to the from
bound and the magnitude of a negative value to the to bound, then records
`READ.MODULE.BRANCH_DUAL_SPLIT`. If a branch had a zero or unlimited rating,
the original pair is not uniquely recoverable; the deterministic split keeps
the signed value without claiming otherwise.

## Differentiability regularization moved to the solver

`ObjectiveTerm::DifferentiabilityRegularization` is removed. It did not name a
portable mathematical quantity or unit. Put numerical regularization in solver
formulation settings and report the declared PowerIO objective separately.

A 0.10 module containing the retired token still decodes. PowerIO removes that
term and adds the warning `READ.MODULE.OBJECTIVE_TERM_RETIRED`; it does not fail
with an opaque unknown enum variant.

## Replacing the network on an instance

Use `DcOpfInstance::with_network` or `AcOpfInstance::with_network` when a
counterfactual changes network parameters. The checked consuming method keeps
the objective, constraints, branch susceptance formula, and any compatible
initial state. This prevents a solution for an amended network from being
attached to an instance rebuilt from defaults.

## DC sign conventions

The public DC susceptance remains negative for an inductive branch. OPF
preparation exposes the distinct positive solver edge weight. The formulas and
matrix shapes are listed in [Matrices and Graphs](matrices.md).

The advanced solver preparation names now state that distinction. Use
`DcBranchParameters`, `DcGeneratorParameters`, and
`NodalGeneratorParameters` in place of the 0.10 `*Data` types. The positive
weight vector is `branches.susceptance_magnitude`, not `branches.b`.
`DcOpfMatrices.bus_branch_incidence` states its bus by branch orientation, and
`DcOpfMatrices.branch_flow_matrix` is the branch by bus factor over those
positive magnitudes. The corresponding calculation is
`calc_branch_flow_matrix`. The DC OPF bundle keeps `BAt.mtx` unchanged and
uses `branch_flow_matrix` as its manifest operator name in place of
`flow_map`.

## Rust callables start with actions

Rust 1.0 uses verb first names and removes the 0.10 forwarding spellings. Use
`list_states` for a typed collection inventory. Electrical calculations use
`calc_branch_susceptance`, `calc_solver_edge_weight`, the `GenCost::calc_quadratic*`
family, `Branch::calc_effective_tap`, `calc_divisible_tap`,
`calc_terminal_charging`, `calc_series_admittance`, and
`calc_total_charging_b`. The related charging, HVDC, transformer, and AC start
calculations follow the same `calc_*` rule. `to_star_expansion` names the
three winding transformer projection.

Matrix helpers now lead with their operation: `calc_diagonal`,
`calc_susceptance_diagonal`, `calc_unit_vector`,
`calc_reference_indicator`, `calc_zero_impedance_skips`,
`calc_matrix_stats_for_kind`, `check_sddm`, `select_solver_for_shape`, and the
`map_*` grounded index methods. In memory serialization uses `to_mtx_bytes`,
`to_vector_mtx_bytes`, and `to_gridfm_record_batches*`; `number_snapshots`
stamps the scenario identifiers.

Component format names parse to typed enums through `parse_*`; facade artifact
metadata uses `resolve_format`. Geographic projections use
`to_geo_layer_from_pwd`, the facade owned `to_geo_layer_from_aux_text`, and
`to_lonlat_from_pwd_mercator`. GridFM discovery uses `list_*` and the base case
uses universal `parse_file` plus `export_state` when the parsed value is a
scenario set. Diagnostic projections use `render_*` or `to_*`.
The released noun and adjective spellings do not remain as 1.0 aliases. The
[1.0 API surface](final-v1-api-cleanup.md) lists every removed source name and
its replacement. C ABI 6 keeps all released symbols.

Applications that need a canonical file name after selecting an output format
use `resolve_format`. It resolves aliases without exposing the component
`TargetFormat` enum and reports `token`, `extension`, `is_directory`, and
`can_emit` in Rust, Python, Julia, and C.

At the Rust facade root, `parse_display_file` now returns `powerio::Error`
rather than `powerio_tx::Error`. The facade no longer exports
`to_geo_layer_from_aux_substations(&AuxFile)`, whose argument type was not
available there. Use `to_geo_layer_from_aux_text`; parser authors that already
hold an `AuxFile` can import the original operation from `powerio-tx`.
