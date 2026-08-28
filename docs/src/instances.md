# Problem Instances and Solutions

An instance is the complete input for one named calculation. A solution is the result of one, and shares the instance it solves. Seven calculation families exist for each, from DC power flow through multiconductor AC OPF to AC security constrained unit commitment.

A source parses to an instance or solution only when it declares that calculation:

| Source | Parses to | Why |
|---|---|---|
| DOE GO Challenge 3 JSON | `ac_scuc_instance` | one coupled scheduling horizon with commitment, reserves, and contingencies |
| BMOPF JSON | `mc_ac_opf_instance` | a multiconductor OPF with limits, costs, and an objective |
| DeepMind OPFData JSON | `ac_opf_solution` | a solved AC OPF with its stated solution values |

A MATPOWER case or PowerModels file stays a network: it carries ratings and costs that power flow, DC OPF, and AC OPF can all use, and nothing in the file commits the caller to one calculation.

The instance contains or shares its network, and asking for only the network is an explicit step that reports the calculation data it discards:

```julia
using PowerIO
scuc = parse_file("scenario_002.json")   # PioModule{AcScucInstance}
inspect(scuc)                            # counts, periods, the declared calculation
write_json(scuc)                         # any kind stores as .pio.json
```

Solvers consume instances; PowerIO never solves. Tellegen, ExaModelsPower, and other solvers take the instance's data, choose their own equations or relaxations, and return results. The instance carries the mathematical input; the equation choice (B-theta against PTDF for DC OPF, polar against SOC for AC OPF) belongs to the solver and does not create another instance family.

Solutions record values by stable element identity, the termination claim, and numerical residuals PowerIO computes rather than trusts. An OPFData solution exposes the instance it solves; residual checks run against that instance's own network.
