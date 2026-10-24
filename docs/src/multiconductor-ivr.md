# Multiconductor IVR preparation

`powerio_matrix::build_mc_ac_opf_preparation` converts an `McAcOpfInstance`
into solver-independent rectangular voltage/current coefficients. It occupies
exactly the same boundary as balanced AC and DC OPF preparation: `powerio-dist`
owns the electrical network, `powerio-prob` owns the instance and selections,
and `powerio-matrix` derives indexed numerical data. The `powerio::matrix`
facade exposes the same API. No optimizer, AD library, or expression type crosses
this boundary.

Choose explicit voltage and per-coil power bases with
`McAcOpfAssemblyOptions::new`, then pass the instance and bases to
`build_mc_ac_opf_preparation`. The
[API example](https://eigenergy.github.io/powerio/powerio_matrix/fn.build_mc_ac_opf_preparation.html#examples)
shows the call and is compiled by the crate's documentation tests.

## Units and axes

Canonical inputs are SI. Explicit bases are voltage in V and power in VA per
coil. Current base is `S_base / V_base`; impedance base is `V_base² / S_base`.
The output stores per-unit voltages, currents, powers, impedance, and admittance.
Cost coefficients are currency/hour per per-unit active injection, including the
instance objective weight. Angle bounds and offsets are radians. Complex values
are `[real, imaginary]`; magnitude bounds are not squared.

The terminal axis follows canonical bus storage order and each bus's terminal
order. Every terminal carries its original bus/name identity. Branch matrices
are local square conductor matrices; `from` and `to` map those axes into the
global terminal axis. Device coils follow their connection incidence, while
`terminals` retains the source map including a neutral. `source_row` indexes the
canonical table identified by the device kind. Transformer current indices are
local to `coils`; equation voltage indices are global. Ports reconstruct rated
physical currents and voltages and need not coincide with internal coil axes.

Star-device current ratings follow `terminal_map`, including a neutral at any
position. Preparation maps each rating to its physical terminal before forming
phase coils. A two-wire coil uses the tighter outgoing/return rating; delta
ratings remain coil ordered. Missing or infinite caps remain unbounded, and zero
remains an exact zero-current bound.

Load power is positive consumption; generator, IBR and source power is positive
injection. All limits distinguish absent from exact zero. Deselecting a bound
does not remove a physical device law or infer an unprovided bound. Voltage
selections use bus names, conductor selections use `line:name`, `switch:name`,
and `transformer:name`, and capability selections use generator names and
`ibr:name`.

## Supported exact profile

Preparation retains mutual line impedance and both end shunts, explicit neutral
conductors, shunts, capacitor matrices, and ideal switches/sources. Currents
remain explicit at exact-zero or singular impedance. There is no neutral
reduction, impedance inversion, epsilon impedance, or ideal-switch contraction.

Supported devices include WYE/DELTA/two-terminal P/I/Z/ZIP/exponential loads,
generator capability and linear dispatch cost, single-phase/center-tapped/Yd/Dy
and fixed n-winding transformers, type-A/B regulators, and supported open-delta
banks. Winding descriptors preserve neutral/core paths and physical current/VA
port limits. Ordinary two-sided transformer and regulator taps can be continuous;
arbitrary n-winding optimized taps are outside this profile.

Supported IBR profiles include single-phase, three-leg and four-leg topology,
PQ/current/apparent-power bounds, signed fixed PF, supported PG/PN/PP volt-var
and volt-watt curves (per-phase or averaged), and isolated DC-link active balance.
Prepared control knots and load power-law terms describe the laws without
choosing a derivative implementation. Scalar canonical capacitor nameplates
follow `DistCapacitor`; BMOPF per-coil capacitor arrays arrive as exact terminal
shunts, shared with other preparation and matrix consumers.

Fixed-PF and volt-var laws intersect the selected reactive-power capability box.
Preparation retains both even when their intersection is infeasible; it does not
silently relax the capability or clamp the control law. Deselecting generator
capability removes the box while preserving the physical control law.

## Formulation restrictions and downstream responsibility

This builder rejects initial points, terminal-referenced voltage sources,
floating source neutrals, unreferenced islands, ideal conductor cycles/paths
between fixed references, and angle windows outside the strict (-pi/2, pi/2)
domain. Those are preparation-profile
restrictions, not canonical network invalidity or proofs of infeasibility.

Earth-referenced source phasors prescribe absolute terminal voltages. A source
with an explicit reference terminal prescribes a voltage difference; this IVR
profile rejects it through the shared source-reference readiness check rather
than interpreting it as an absolute voltage.

Explicit DC networks, uninterpreted electrical extras, conflicting or unsupported
IBR policies, and unimplemented tap profiles also fail explicitly. The builder
checks finite dimensions, units, source identities, and selections before
returning data. Public records are derived arrays, not a new portable IR value
or a stable serialized solver model; reconstruct them from the canonical instance.

Transformer rows are `equations[k] + t * tap_equations[k] = 0`, with nominal
multiplier `t = 1`. The physical tap is `tap_nominal * t`, or `tap_nominal / t`
for the inverse regulator convention. Fixed taps have no multiplier rows.
Initial voltage seeds are suggestions; a free common mode in a seed is not a
physical reference imposed on the optimization problem. With transformers
present, the seeds come from a dense solve of the no-load voltage equations,
whose memory grows with the square of the terminal count and whose time grows
with its cube. Feeders beyond about ten thousand terminals can need
gigabytes; a sparse seed solve is planned.

The downstream solver owns expression construction, objective/constraint
scaling, sparse Jacobians/Hessians, optimization, status acceptance, and independent
physical residual validation. Polynomial rows admit exact affine/quadratic
coefficients; nonpolynomial control/load laws may use the solver's existing AD.
PowerIO does not prescribe or depend on that choice, return an optimum, or expose
prices or solution sensitivities.

## Verification

Preparation tests check hand-computed SI/per-unit values, explicit incidence,
zero/absent bounds, disabled selections, malformed metadata, canonical edits,
source coverage and unsupported profile boundaries. Converter tests independently
check capacitor susceptance, custom open-delta maps and winding limits. Y-bus,
LinDist3Flow and IVR consumers have separate capacitor witnesses.

The companion Tellegen suite checks equation residuals, analytical/AD/finite-
difference derivatives, feasibility, dispatch changes under reversed costs,
and 64 frozen solves against pinned BMOPFTools IVR. These are downstream evidence;
ordinary PowerIO tests require neither Julia nor an optimizer. Matching an
objective alone does not establish correct physics or a global optimum.
