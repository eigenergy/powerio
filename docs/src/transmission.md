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

## Merging buses

A closed switch or a zero impedance branch joins two buses into one
electrical node, which no finite admittance describes. `merge_buses` resolves
them explicitly: each set of joined buses becomes one bus, every element moves
onto it, and the joining elements are removed. The rule states what joins
buses, and no threshold applies unless the rule names it.

| Rule | Merges |
|---|---|
| closed switches | the two buses of every closed switch |
| exact | branches with `r = 0` and `x = 0` (a transformer only at nominal ratio with no shift) |
| PSS/E threshold | non-transformer branches with `r = 0` and `abs(x)` at most the threshold, by default the `THRSHZ` the case states |
| impedance magnitude | non-transformer branches with `abs(r + jx)` at most the threshold |

Every rule considers in-service branches only. The surviving bus of a set is
its reference bus, else a bus with an in-service generator, else a bus a
generator regulates, with the smallest id breaking each tie. Loads, shunts,
generators and their regulated buses, storage, branch, switch, and HVDC ends,
three winding transformer windings, control buses, and area swing buses all
follow the survivor. The result lists every removed element with its source
row, identity, and ratings, and maps each source branch and switch row to its
merged row. A removed branch's line charging and line shunts become a fixed
shunt at the survivor, so the merged network draws the reactive power the
unmerged one did; `charging="drop"` discards them instead. A branch the merge shorts, such as a line in parallel with a
jumper, is removed and reported. A jumper whose merge would join two windings
of one three winding transformer stays, also reported. Merging the result
again changes nothing.

The removed elements' flows are not variables of the merged network.
`calc_removed_flows` recovers their active power from a solution of the merged
network. Kirchhoff's current law fixes the flows where the removed elements
form a tree, the elements' reactances split a loop, and a loop of zero
reactance elements, such as a ring of closed switches, gets the minimum norm
split and a diagnostic.

```python
merge = network.merge_buses(zero_impedance="psse")
merged = merge.network                 # solve this network
flows = merge.calc_removed_flows(p_from, p_to)
```

```sh
powerio summary case.raw --merge-buses psse
powerio convert case.raw --merge-buses psse=0.0001 --to matpower -o merged.m
```

## Islands and reference buses

A large case can hold several AC islands: systems joined only by HVDC,
pockets behind an open breaker, and stranded equipment. `calc_islands`
partitions the energized buses into islands joined by in-service branches,
closed switches, and in-service three winding transformers. Buses typed
isolated belong to no island, and HVDC lines do not join islands. Each island
lists its buses, reference buses, and in-service generators, largest island
first. `subset_buses` carves one island out as a network of its own.

A power flow needs one reference bus per island, and an island with no source
cannot be solved. `assign_island_references` with the per island policy gives
each island exactly one:

- an island that states one reference keeps it;
- an island that states none takes the bus of its largest `pmax` in-service
  generator;
- an island that states several keeps the one hosting the most generation and
  demotes the others;
- an island with no in-service generator is de-energized: its buses are typed
  isolated and the equipment on or touching them is taken out of service.

Every change is reported under `CANONICALIZE.ISLAND`. Normalization applies
the same rule when asked, and leaves the unsupplied islands out of the
normalized network; by default it keeps the case's references and designates
one only when none survives. `powerio summary` lists the islands under
`topology.islands`.

```python
normalized = network.to_normalized(island_references="per_island")
```

Building matrices from a balanced network has its own chapter,
[Matrices and graphs](matrices.md).
