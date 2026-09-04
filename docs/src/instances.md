# Calculation instances and solutions

An instance is the complete input for one named calculation. A solution is
the result of one and shares the instance it solves. PowerIO has seven
instance types, from DC power flow through multiconductor AC OPF to AC
security constrained unit commitment, and eight solution types, because
`SocwrOpfSolution` records an SOCWR relaxation of an `AcOpfInstance` without
labeling it an AC OPF solution.

```text
DcPfInstance       DcPfSolution
AcPfInstance       AcPfSolution      SocwrOpfSolution
DcOpfInstance      DcOpfSolution
AcOpfInstance      AcOpfSolution
McAcPfInstance     McAcPfSolution
McAcOpfInstance    McAcOpfSolution
AcScucInstance     AcScucSolution
```

A source parses to an instance or a solution only when it declares that
calculation:

| Source | Parses to |
|---|---|
| DOE GO Challenge 3 problem file | `AcScucInstance` |
| that problem file beside its solution file | `AcScucSolution` |
| DeepMind OPFData JSON | `AcOpfSolution` |
| BMOPF JSON | `MulticonductorNetwork`; construct the instance explicitly |

A MATPOWER case or a PowerModels file stays a network. It carries ratings
and costs that power flow, DC OPF, and AC OPF can all use, and nothing in the
file commits the caller to one calculation. The GO Challenge 3 data format
also serves later challenges; only its Challenge 3 problem file defines a
calculation PowerIO types, and optional fields outside that formulation are
retained in the source and reported.

GO Challenge 3 splits its problem and solution across two files. Put both in
one directory and parse the directory:

```python
solution = powerio.parse("scenario_002")
```

A directory holding only the problem file returns `AcScucInstance`. A
solution file alone is refused, because it contains neither the component
definitions nor the time axis. The returned module retains both files and its
diagnostics.

A DC instance names its branch susceptance formula. Rust selects it with
`with_branch_susceptance_formula(formula)` and reads it with
`branch_susceptance_formula()`; the PowerIO IR document stores it in the
`approximation` field as one of `series_susceptance`,
`tap_adjusted_reactance`, or `reactance_only`. [Matrices and
graphs](matrices.md) defines the three.

The instance contains or shares its network. `emit` writes that network in a
grid exchange format and reports the calculation fields the format cannot
carry. `serialize` preserves the complete instance:

```julia
using PowerIO
scuc = parse("scenario_002")             # PioModule{AcScucInstance}
emit(scuc, "matpower", "scenario_002.m") # diagnostic: scheduling data omitted
serialize(scuc, "scenario_002.pio.json")
```

Solvers consume instances; PowerIO never solves. The instance carries the
mathematical input. The equation choice, B-theta against PTDF for DC OPF or
polar against SOC for AC OPF, belongs to the solver and does not create
another instance family.

A solution records values by stable element identity, the termination claim,
and residuals PowerIO computes rather than trusts. An OPFData solution
exposes the instance it solves, and residual checks run against that
instance's network. Emitting a solution writes the bus voltages, generator
dispatch, and branch terminal flows the target format supports and reports
the omitted objectives, multipliers, termination data, and residuals. An
SOCWR result is never written as an AC power flow solution: `emit` writes
only its instance network and reports that the W-space values and the
objective lower bound were omitted.
