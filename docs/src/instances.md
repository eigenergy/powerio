# Calculation instances and solutions

An instance is the complete input for one named calculation. A solution is the result of one, and shares the instance it solves. PowerIO has seven instance types, from DC power flow through multiconductor AC OPF to AC security constrained unit commitment. It has eight solution types because `SocwrOpfSolution` records an SOCWR relaxation of an `AcOpfInstance` without mislabeling that result as an AC OPF solution.

A source parses to an instance or solution only when it declares that calculation:

| Source | Parses to | Why |
|---|---|---|
| DOE GO Challenge 3 input/problem data file | `AcScucInstance` | one coupled AC scheduling horizon with commitment, reserves, and contingencies |
| BMOPF JSON | `MulticonductorNetwork` | a conductor resolved network; construct the required calculation instance explicitly |
| DeepMind OPFData JSON | `AcOpfSolution` | a solved AC OPF with its stated solution values |

A MATPOWER case or PowerModels file stays a network: it carries ratings and costs that power flow, DC OPF, and AC OPF can all use, and nothing in the file commits the caller to one calculation.

The GO Challenge 3 format and the Challenge 3 calculation are not synonyms.
The data format document describes a format “for Challenge 3 and beyond” and
marks some fields as optional data outside the Challenge 3 formulation. A
Challenge 3 input/problem data file parses to
`PioModule<AcScucInstance>` because its required inputs define that
calculation. Optional fields for later Challenges or analysis stay outside the
SCUC instance unless PowerIO has a source neutral type with the same meaning.
PowerIO retains the original source for exact same format emission and reports
present fields that have no typed representation. A later use of the GO data
format that defines another calculation must map to that calculation's own instance,
not be forced into `AcScucInstance` because it uses related JSON fields.

GO Challenge 3 separates its problem and solution into two files. Put both in
one directory and parse the directory:

```python
solution = powerio.parse("scenario_002")
```

The parser identifies each file by its required top level fields. A directory
with only the problem file returns `PioModule<AcScucInstance>`. Adding the
matching solution file makes the same operation return
`PioModule<AcScucSolution>`. A solution file alone is incomplete because it
does not contain the component definitions or time axis. The returned module
retains both files and its diagnostics.

Rust selects a DC branch formula with
`with_branch_susceptance_formula(formula)` and reads it with
`branch_susceptance_formula()`. PowerIO IR v0.11.0 documents retain the
field `branch_susceptance_formula`. Its value is a
`BranchSusceptanceFormula` such as `series_susceptance`.

The instance contains or shares its network. `emit` can write that network to
a grid exchange format and reports the calculation fields that format cannot
carry. `serialize` preserves the complete instance:

```julia
using PowerIO
scuc = parse("scenario_002.json")       # PioModule{AcScucInstance}
emit(scuc, "matpower", "scenario_002.m") # diagnostic: scheduling data omitted
serialize(scuc, "scenario_002.pio.json")
```

Solvers consume instances; PowerIO never solves. Tellegen, ExaModelsPower, and other solvers take the instance's data, choose their own equations or relaxations, and return results. The instance carries the mathematical input; the equation choice (B-theta against PTDF for DC OPF, polar against SOC for AC OPF) belongs to the solver and does not create another instance family.

Solutions record values by stable element identity, the termination claim,
and numerical residuals PowerIO computes rather than trusts. An OPFData
solution exposes the instance it solves; residual checks run against that
instance's own network. Emitting a solution writes the bus voltages,
generator dispatch, and branch terminal flows the target format supports and
diagnoses omitted objectives, multipliers, termination data, and residuals.
An SOCWR result is never written as an AC power flow solution: `emit` writes
only its instance network and reports that the W-space values and objective
lower bound were omitted.
