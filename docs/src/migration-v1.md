# Migrating from 0.10

PowerIO 1.0 corrects OPF preparation and solution contracts found during the
0.10 consumer integration. The `.pio.json` schema remains `powerio.module/1`,
but the stored OPF result fields and the Rust API changed.

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

`nodal_generator_data` now returns `Result`. It rejects a piecewise curve
because one nodal quadratic cannot represent it. `write_dcopf_bundle` uses that
projection and returns the same error; use the generator space preparation for
an exact piecewise objective.

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
the objective, constraints, DC approximation, and any compatible initial
state. This prevents a solution for an amended network from being attached to
an instance rebuilt from defaults.

## DC sign conventions

The public DC susceptance remains negative for an inductive branch. OPF
preparation exposes the distinct positive solver edge weight. The formulas and
matrix shapes are listed in [Matrices and Graphs](matrices.md).
