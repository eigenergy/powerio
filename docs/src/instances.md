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
`approximation` field as one of `series_susceptance`,
`tap_adjusted_reactance`, or `reactance_only`. [Matrices and
graphs](matrices.md) defines the three.

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
one costs, budgets on open branches and on status changes, whether islands may
form, and which branch current limits apply. `to_ac_ots_instance` builds the
default from a network or an AC OPF: every initially in-service branch may
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
