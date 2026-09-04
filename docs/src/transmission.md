# Transmission networks

A balanced network is the positive sequence transmission model. Every
balanced format in the [format table](format-fidelity.md) parses to it, so
one reader and one writer per format serve every conversion.

```julia
using PowerIO
module_ = parse("case118.m")       # PioModule{BalancedNetwork}
net = module_.value
length(net.buses)                  # 118
net.branches[1]                    # buses, branches, generators, loads, and the other tables
```

The network keeps what the source states: the element inventory, terminal
connections, impedances, ratings, generator capability bounds and cost
curves, and the source's operating assignment. Ratings and costs stay on the
network as reusable data. Which bounds a calculation enforces is decided when
an instance is constructed, so one parsed case serves power flow, DC OPF, and
AC OPF without reparsing ([Calculation instances and solutions](instances.md)).

Conventions the accessors hold to:

- bus identifiers are the source's own; dense zero-based indices exist only
  in matrix results, which carry the mapping;
- powers are MW and MVAr as the source states them, angles degrees;
  `to_normalized` derives a per unit, radian, in service only copy when a
  solver wants one;
- a branch `tap` of `0` means `1`, and a `rate_a` of `0` means unrated.

Sources that state more than the balanced calculation view, such as XIIDM,
CGMES, and PSS/E RAW 35, also fill the detailed connectivity: substations,
voltage levels, connectivity nodes, terminals, switches, operational limit
groups, and tap changer controls. A writer that can state those records
writes them; one that cannot reports what it leaves out.

```julia
emit(module_, "matpower", "copy.m")            # the source bytes, unchanged
result = emit(module_, "psse")                 # fresh PSS/E text
for finding in result.diagnostics
    println(finding.code, ": ", finding.message)
end
```

Library callers compose `parse` and `emit` and keep the module between them.
The command line has the one call form `powerio convert`.

Matrix construction from a balanced network is its own chapter:
[Matrices and graphs](matrices.md).
