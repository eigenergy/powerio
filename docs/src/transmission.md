# Transmission Networks

A balanced network is the positive sequence transmission model. MATPOWER, PSS/E, PowerWorld AUX and PWB, PSLF EPC, PowerModels JSON, Egret JSON, pandapower JSON, Surge JSON, and the PyPSA CSV electrical profile all parse to it, so one workflow serves every balanced source and a new format needs one parser rather than pairwise converters.

```julia
using PowerIO
case = parse("case118.m")          # PioModule{BalancedNetwork}
net = case.value
n_buses(net)                       # 118
net.data.branches[1]               # element tables: buses, branches, generators, …
```

The network keeps what the source states: element inventory, terminal connections, impedances, ratings, generator capability bounds and cost curves, and the source/default operating assignment. Ratings and costs stay on the network as reusable data; selecting which bounds a calculation enforces happens when an instance is constructed, so one parsed case serves power flow, DC OPF, and AC OPF without reparsing ([Problem Instances and Solutions](instances.md)).

Conventions the accessors hold to:

- bus identifiers are the source's own (MATPOWER buses are 1 based); dense zero based indexing exists only in matrix results, which carry the mapping;
- powers are MW and MVAr as the source states them, angles degrees; `to_normalized` derives a per unit, radian, in service only copy when a solver wants one;
- a branch `tap` of `0` means `1`; `rate_a` of `0` means unrated.

Writing an unchanged parsed module back to its own format is byte exact, comments and field layout included. Converting to another balanced format keeps everything the target can represent and reports the rest:

```julia
emit(case, "matpower", "copy.m")               # byte exact echo
result = emit(case, "psse")                     # conversion + reported losses
for finding in result.diagnostics
    println(finding.code, ": ", finding.message)
end
```

Library callers compose `parse` and `emit`, keeping the module and its
diagnostics available between the operations. The command line keeps the one
call `powerio convert` form.

Matrix construction from a balanced network is its own chapter: [Matrices and Graphs](matrices.md).
