# Multiconductor LinDist3Flow design review

Status: pre-merge acceptance review, 2026-09-10. This record reviews the
implementation on `codex/lindist3flow` against issue
[#151](https://github.com/eigenergy/powerio/issues/151). It was written after
the exploratory commits, not before them; that chronology is retained rather
than presenting this as the pre-implementation note requested by the issue.
The branch must not merge until this review and its validation evidence have
been accepted.

## Decision

LinDist3Flow is a formulation over `MulticonductorNetwork`. It does not pass
through `BalancedNetwork`, scalar branch `r/x`, or a hidden positive-sequence
equivalent. Explicit-neutral elimination is a separate, auditable same-family
network projection. PowerIO constructs and compiles the problem but does not
solve it.

The ownership boundary is:

```text
MulticonductorNetwork
        |
        | optional/required neutral_kron_reduce projection
        v
MulticonductorNetwork + NeutralKronReport
        |
        | powerio-prob applicability, topology and reference
        v
LinDist3FlowOpfInstance
        |
        | powerio-matrix affine/SOC compilation and scaling
        v
P, q, A, b, K + semantic row/column identities
        |
        | consumer-owned solver adapter
        v
LinDist3FlowOpfSolution
```

This keeps physical network transformations in `powerio-dist`, matrix-free
problem meaning in `powerio-prob`, sparse numerical representation in
`powerio-matrix`, and solver policy outside PowerIO. Tellegen's adapter is one
consumer; the standard form remains usable by any native or WebAssembly conic
solver.

## Neutral projection

`powerio_dist::neutral_kron_reduce` returns a new network and never mutates its
input. For each referenced series-impedance primitive it eliminates one
identified neutral by the exact Schur complement

```text
Z_reduced = Z_pp - Z_pn Z_nn^-1 Z_np.
```

It rewrites conductor-indexed terminal maps, ratings and bounds, and records
each bus decision and component rewrite in `NeutralKronReport`. It also records
the current recovery map `i_n = -Z_nn^-1 Z_np i_p`. A neutral must be explicitly
grounded unless the caller opts into the physical ideal-ground approximation.
Ambiguous neutrals, inconsistent conductor positions, singular `Z_nn`, and
unsupported conductor-indexed data, including finite current limits on the
eliminated neutral, are errors. The recovery map produces neutral current
under zero neutral voltage drop; it does not recover a neutral voltage.

The reduced network carries machine-readable provenance under
`powerio_neutral_kron`, in addition to the typed `NeutralKronReport`.
`LinDist3FlowBuildOptions::require_neutral_provenance` lets an application
require that evidence. Instance construction rejects any remaining conductor
identified as a neutral by the network conventions; callers can provide
explicit per-bus neutral labels to the projection when source conventions are
ambiguous.

This projection is intentionally not a general neutral model: it does not
retain neutral voltage as an optimization variable and does not approximate a
floating or impedance-grounded neutral by default.

## Current supported formulation

The strict initial slice has the following contract. `Reject` is the only
implemented unsupported-data policy; the other enum values reserve future API
space and currently make the instance inapplicable.

| Concern | Current contract |
|---|---|
| Network family | `MulticonductorNetwork` only. No balanced conversion. |
| Topology | Cycles and parallel lines are retained in the conductor-resolved graph. Each physical connected component must be source covered, and multiple voltage-source records on one physical island are rejected. Lines receive a deterministic source-rooted orientation without changing source row identity. A warning identifies meshed instances. |
| Conductors | All retained bus terminals must be phase conductors. Line terminal maps must be nonempty, equal width and resolve exactly at both ends. Mutual series impedance is retained. |
| Reference | `Auto` uses a complete initial operating point when available, otherwise propagates source phasors at no load. `Explicit` requires the initial point; `SourcePropagated` ignores it. Every reference phasor must be finite and nonzero. Angles are fixed in the linearization. |
| Lines | Full finite conductor series-impedance matrices and length are supported. Pi shunt admittance is rejected. Apparent-power and current limits are selected from the base instance constraints. |
| Voltage bounds | Per-terminal phase-to-ground magnitude bounds become bounds on squared voltage. Source-terminal squared voltages are fixed to the reference values. |
| Loads | Constant-power and constant-impedance models are supported. ZIP is supported only when both active and reactive constant-current fractions are zero. Constant-current, exponential and other load models are rejected. |
| Connections | Grounded-wye channels, one-channel single phase across one or two terminals, two-terminal delta and three-channel/three-terminal delta are supported. The terminal power split is frozen at the voltage reference. Other arities/configurations are rejected. |
| Shunts | Finite square terminal admittance matrices are supported through a fixed-angle affine voltage closure. |
| Generators | Per-channel active/reactive dispatch, paired bounds, apparent-power limits, current limits and linear energy cost are supported for the connection forms above. If selected bounds are absent, nominal dispatch is fixed. |
| Voltage sources | Per-terminal active/reactive injection variables and optional linear energy prices are supported. Source voltage is fixed. |
| Transformers and regulators | Rejected. No tap, winding, phase-shift or regulator lowering is implicit. |
| Switches | Rejected regardless of state. A future lowering pass may remove open switches and contract closed switches, but that must be explicit and audited. |
| Other equipment | Capacitors, IBRs and untyped objects are rejected. Capacitors are not silently treated as generic shunts. |
| Objective | Feasibility or exactly one `ActivePowerDispatchCost` term. Costs are linear and expressed in the multiconductor device data; balanced-network polynomial generator costs are not compiled. |
| Time | One snapshot. No time coupling, switching decision or discrete variable. |

Rejecting an element means applicability returns an error diagnostic before
coefficient construction. It does not mean the element is ignored.

## Equations and approximation boundary

The decision variables are retained-terminal squared magnitudes `w`, line
conductor active/reactive powers `p/q`, and generator/source channel powers.
For line impedance `Z` and reference phasor `v_bar`, coefficient blocks are

```text
Gamma[phi, psi] = v_bar[phi] / v_bar[psi]
M =  2 Re(conj(Z) .* Gamma)
N = -2 Im(conj(Z) .* Gamma)
w_child = w_parent - M p - N q.
```

The model is lossless: the same line `p/q` enters the parent and child nodal
balances. Cross-voltage products use the first-order fixed-angle closure of
`sqrt(w_phi w_psi) exp(j delta_theta)`. Connection power maps and winding
voltage expressions are likewise frozen or linearized at the reference.
These are formulation approximations, not parser or unit conversions.

Meshed instances retain these nodal balances and one squared-voltage drop row
for every line conductor. They add no voltage-angle recovery,
loop-consistency, circulating-flow penalty, or automatic radialisation. A
feasible mesh can therefore have nonphysical or non-unique active/reactive
flow allocations. The mesh diagnostic and `LinDist3FlowTopology::meshed`
metadata make that limitation explicit; successful optimization is only a
certificate for the assembled approximate model.

Selected apparent-power limits use

```text
p^2 + q^2 <= S_max^2,
```

and selected current limits use the rotated-cone representation

```text
p^2 + q^2 <= w I_max^2
```

at both line endpoints and at generator winding voltages. The standard-form
compiler maps rotated cones to ordinary second-order cones, retains semantic
row origins, and defaults to diagonally scaled per-unit solver coordinates.
Decoded solution values are returned in SI units.

## Provenance of the implementation

No code was copied forward from stale draft PR #139. The implementation was
written against the current `powerio-dist` model and tests, with the formulation
cross-checked against the newer PowerOptLab.jl LinDist3Flow work in PR #53.
Any future recovery of code or behavior from #139 requires a new review against
the then-current `MulticonductorNetwork` semantics.

## Validation evidence

The validation layers intentionally test different claims:

| Evidence | Claim checked |
|---|---|
| `powerio-dist/tests/kron.rs` | Schur-complement values, terminal/data rewrites, provenance, recovery coefficients, and refusal of ambiguous or non-ideal neutral assumptions. |
| `powerio-prob/tests/lindist3flow.rs` | Conductor radiality, source coverage, reference selection, connection/device dimensions and explicit unsupported-equipment diagnostics. |
| `powerio-matrix` unit tests | Cross-voltage, winding, connection, coupled voltage-drop, device balance, cone assembly, scaling and primal decoding formulas. |
| `powerio-matrix/tests/lindist3flow.rs` BMOPF case | End-to-end explicit-neutral parse, Kron projection, standard-form feasibility, SI decoding and objective for a hand-checkable feeder translated from the PowerOptLab reference. |
| `lindist3flow_oracle.dss` and its JSON result | Independent OpenDSSDirect.py 0.9.4 nonlinear solve of the matching one-phase reduced feeder. The Rust regression requires the compiled lossless-linear solution to remain feasible and its voltage-magnitude error to remain below 0.11%. |
| `evals/validation/validate_lindist3flow_opendss.py` | Regenerates every committed OpenDSS value, preventing the external oracle from becoming an unaudited literal. |
| Tellegen native and WebAssembly tests | Clarabel consumes the portable sparse form and the decoded solution/objective agree in native and browser-compatible builds. |

The OpenDSS comparison is an approximation bound, not a claim that a
lossless linear model reproduces nonlinear losses. For the oracle feeder,
OpenDSS gives 220.193085863 V and LinDist3Flow gives 220.426858618 V at the
load, a 0.1062% magnitude error. OpenDSS line sending power includes 420.419 W
and 210.209 var of losses; LinDist3Flow intentionally balances the 10 kW and
2 kvar load without those losses.

## Deferred work

The following require explicit design and tests rather than relaxing the
applicability gate:

- switch-state lowering and topology provenance;
- transformer and regulator affine models, including tap and phase mappings;
- line charging or endpoint-shunt lowering;
- constant-current and exponential load approximations;
- floating/impedance-grounded neutral models;
- capacitor and IBR formulation support;
- angle recovery, loop-consistency equations, circulating-flow regularisation,
  automatic radialisation, and discrete controls;
- broader unbalanced three-phase OpenDSS/PowerModelsDistribution oracle cases.

The first external oracle is deliberately minimal and hand inspectable. It
closes the absence of an independent solve comparison; it does not replace the
need for broader feeder validation as each deferred feature becomes supported.
