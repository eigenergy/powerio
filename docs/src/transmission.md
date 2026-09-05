# Transmission networks

A balanced network is the positive sequence transmission model. All of the
balanced formats in the [format table](format-fidelity.md) parse to it, so
one reader and one writer per format cover all the conversions between them.

```julia
using PowerIO
module_ = parse("case118.m")       # PioModule{BalancedNetwork}
net = module_.value
length(net.buses)                  # 118
net.branches[1]                    # buses, branches, generators, loads, and the other tables
```

The network keeps what the source says: the element inventory, terminal
connections, impedances, ratings, generator capability bounds and cost
curves, and the source's operating assignment. Ratings and costs stay on the
network as reusable data, and which bounds a calculation enforces is decided
when you construct an instance, so one parsed case serves power flow, DC OPF,
and AC OPF without reparsing (see
[Calculation instances and solutions](instances.md)).

A few conventions hold across the accessors. Bus identifiers are the source's
own; dense zero based indices exist only in matrix results, which include the
mapping. Powers are MW and MVAr as the source gives them, and angles are
degrees; `to_normalized` derives a per unit, radian, in service only copy
when a solver wants one. A branch `tap` of `0` means `1`, and a `rate_a` of
`0` means unrated.

Sources that describe more than the balanced calculation view, such as XIIDM,
CGMES, and PSS/E RAW 35, also fill in the detailed connectivity: substations,
voltage levels, connectivity nodes, terminals, switches, operational limit
groups, and tap changer controls. A writer whose format can represent those
writes them, and one whose format cannot reports what it left out.

```julia
emit(module_, "matpower", "copy.m")            # the source bytes, unchanged
result = emit(module_, "psse")                 # fresh PSS/E text
for finding in result.diagnostics
    println(finding.code, ": ", finding.message)
end
```

From a library you call `parse`, keep the module, and call `emit`; on the
command line, `powerio convert` does both in one call.

Building matrices from a balanced network has its own chapter,
[Matrices and graphs](matrices.md).
