# Calculation instances and solutions

An instance is the complete input for one named calculation, and a solution
is the result of one; the solution shares the instance it solves. PowerIO has
ten instance types, from DC power flow through AC optimal transmission
switching, LinDist3Flow, and multiconductor AC OPF to AC security constrained
unit commitment, and eleven solution types, because
`SocwrOpfSolution` is an SOCWR relaxation of an `AcOpfInstance` and is not
labeled an AC OPF solution.

```text
DcPfInstance       DcPfSolution
AcPfInstance       AcPfSolution      SocwrOpfSolution
DcOpfInstance      DcOpfSolution
AcOpfInstance      AcOpfSolution
AcOtsInstance      AcOtsSolution
McAcPfInstance     McAcPfSolution
McAcOpfInstance    McAcOpfSolution
LinDist3FlowPfInstance   LinDist3FlowPfSolution
LinDist3FlowOpfInstance  LinDist3FlowOpfSolution
AcScucInstance     AcScucSolution
```

A source parses to an instance or a solution only when it declares that
calculation:

| Source | Parses to |
|---|---|
| DOE GO Challenge 3 problem file | `AcScucInstance` |
| that problem file beside its solution file | `AcScucSolution` |
| DeepMind OPFData JSON | `AcOpfSolution` |
| BMOPF JSON or distribution CIM | `MulticonductorNetwork`; construct the instance explicitly |

A MATPOWER case or a PowerModels file stays a network, because it has ratings
and costs that power flow, DC OPF, and AC OPF can all use and nothing in the
file commits you to one calculation. The GO Challenge 3 data format also
serves later challenges, but only its Challenge 3 problem file defines a
calculation PowerIO has a type for; optional fields outside that formulation
are retained in the source and reported.

GO Challenge 3 splits its problem and solution across two files. Put both in
one directory and parse the directory:

```python
solution = powerio.parse("scenario_002")
```

A directory with only the problem file returns an `AcScucInstance`. A
solution file on its own is refused, because it contains neither the
component definitions nor the time axis. The returned module retains both
files and its diagnostics.

A DC instance says which branch susceptance formula it uses. In Rust you
select it with `with_branch_susceptance_formula(formula)` and read it with
`branch_susceptance_formula()`; the PowerIO IR document stores it in the
`approximation` field as one of `tap_adjusted_series_susceptance`,
`series_susceptance_no_phase_shift`, `series_susceptance`,
`tap_adjusted_reactance`, or `reactance_only`. [Matrices and
graphs](matrices.md) defines the five conventions.

An instance contains or shares its network. `emit` writes that network in a
grid exchange format and reports the calculation fields the format has no
place for, while `serialize` preserves the complete instance:

```julia
using PowerIO
scuc = parse("scenario_002")             # PioModule{AcScucInstance}
emit(scuc, "matpower", "scenario_002.m") # diagnostic: scheduling data omitted
serialize(scuc, "scenario_002.pio.json")
```

Solvers consume instances; PowerIO never solves. The instance is the
mathematical input only. The choice of equations, B-theta or PTDF for DC OPF
and polar or SOC for AC OPF, belongs to the solver and does not create
another instance type. LinDist3Flow is an explicit exception to that general
AC choice because its fixed-reference linear approximation, declared mesh
limitations, and applicability decisions define the mathematical problem.

`AcOtsInstance` is an AC OPF whose branch statuses are decisions. It keeps
every physical branch, takes the network's `in_service` bits as the initial
topology, and states which branches may switch, what opening or closing each
one costs, budgets on open branches and on status changes, whether an island
of the initial topology may split, and which branch current limits apply.
`to_ac_ots_instance` builds the default from a network or an AC OPF: every initially in-service branch may
switch, at zero cost. A solver chooses the final statuses;
`fix_ac_ots_topology` derives the AC OPF to solve over them and reports what
that problem leaves out, and `ac_ots_solution` attaches its solution after
checking that it solves exactly that problem and that open branches carry no
flow. The AC OTS solution keeps the instance's network with the initial
statuses; `materialize_ac_ots_solution` returns the network with the final
statuses and solved values for `emit`.

`LinDist3FlowPfInstance` is the fixed-dispatch form: generator and IBR P/Q
must be prescribed, its objective is zero, and retained line thermal limits
are evaluated after convergence instead of constraining the solve. Its
`LinDist3FlowPfSolution` carries those SI-valued monitored checks separately
from solver feasibility.

A solution lists its values by stable element identifier, along with the
termination claim and residuals that PowerIO computes itself rather than
taking on trust. An OPFData solution exposes the instance it solves, and
residual checks run against that instance's network. Emitting a solution
writes the bus voltages, generator dispatch, and branch terminal flows the
target format supports and reports the objectives, multipliers, termination
data, and residuals it left out. An SOCWR result is never written as an AC
power flow solution; `emit` writes only its instance network and reports that
the W-space values and the objective lower bound were omitted.

### Explicit DC conventions

For an inductive branch let `k = x/(r²+x²)`, `τ` be the effective tap
(zero in source data means unity), and `φ` the phase shift in radians.
From-end active power is:

| Token | Flow |
| --- | --- |
| `series_susceptance` (unchanged default) | `k(θf−θt−φ)` |
| `series_susceptance_no_phase_shift` (PowerModels DCP) | `k(θf−θt)` |
| `tap_adjusted_reactance` (MATPOWER / PowerModels DCMP) | `(θf−θt−φ)/(xτ)` |
| `tap_adjusted_series_susceptance` (ignore conductance) | `k(θf−θt−φ)/τ` |
| `reactance_only` | `(θf−θt)/x` |

PowerModels DCP parity assumes the same branch orientation and impedance
reference side. Its parser can reverse an antiparallel transformer, multiplying
`r` and `x` by `τ²`, replacing `τ` by `1/τ`, and negating `φ`. The two
tap-adjusted coefficients are invariant under this transformation; the three
tap-ignoring coefficients change by `1/τ²`. PowerIO keeps the source orientation.
Consequently, selecting the DCP token alone does not guarantee parity with a
PowerModels-parsed file containing reversed off-nominal transformers.

The nodal balance in per unit is
`p_g,i − p_d,i − g_s,i = Σ_from(e)=i f_e − Σ_to(e)=i f_e`, where each `f_e`
includes its convention's phase-shift term and `g_s,i` is the active shunt
withdrawal at unit voltage. When fixed HVDC injections are present, add their
net injection to the left-hand side.

The selected coefficient also scales the nodal phase injection. Thermal ratings
keep their original units and values. These are lossless, cold-start models;
none guarantees the closest AC result on every network. Matching an OPF
benchmark also requires matching its costs, limits, shunts and reference buses.
Older readers reject the new tokens instead of substituting another model.
