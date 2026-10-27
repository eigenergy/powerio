# Transmission networks

A balanced network is the positive sequence transmission model. All of the
balanced formats in the [format table](https://github.com/eigenergy/powerio#formats) parse to it, so
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
generator regulates, with the smallest id breaking each tie. In Rust,
`BusMergeRule::with_survivor` keeps the smallest id or each merged element's
from bus instead, the survivors of `merge_zero_impedance_buses` and
`reduce_zero_impedance`. The survivor takes the strongest bus kind of its set.
Loads, shunts, generators and their regulated buses, storage, branch, switch,
and HVDC ends, three winding transformer windings, control buses, and area
swing buses all follow the survivor. The result lists every removed element
with its source row, identity, and ratings, and maps each source branch and
switch row to its merged row. A removed in-service branch becomes a fixed shunt
with its full equivalent admittance `Yff + Yft + Ytf + Ytt` at the survivor.
This includes the series contribution of a bypassed transformer with an
off-nominal tap or phase shift, and terminal charging with the from side
divided by the tap squared. `charging="drop"` discards only terminal charging;
the series equivalent remains. `charging_shunts` lists all these generated
shunt rows. A branch the merge shorts, such as a line in parallel with a
jumper, is removed and reported. Bypassing an ideal transformer with a
nonunity tap or phase shift returns an error: no finite shunt can preserve
that voltage constraint. A jumper whose merge would join two windings of one three winding
transformer stays, also reported. Merging the result again changes nothing.

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
lists its buses, reference buses, in-service generators, and the in-service
HVDC lines that tie it to another island, largest island first.

`subset_buses(&buses)` returns a `BusSubset`: the cut `network` and a map for
all eleven tables (buses, branches, switches, three winding transformers,
generators, loads, shunts, SVCs, storage, HVDC lines, and areas). Each map has
one entry per source row: `Some(row)` points into the result table and `None`
means that row was excluded. IDs are preserved. For example,
`cut.branch_rows[source_row]` locates a statement's branch in `cut.network`
without reproducing the cut's filtering rules.

A dead winding can bring its bus along as an isolated stub: its `bus_rows`
entry is present, while injections excluded from that stub map to `None`.
Detailed connectivity follows the cut, including calculated buses, topology
connections, equipment metadata and remote regulation references. Unassigned
source topology remains when no calculated bus identifies it as excluded.
Empty voltage levels are pruned and nodes stranded by a cut retain explicit
calculated-bus records so fresh XIIDM emission preserves the selected buses.
The older selector-based `subset` shares this filtering but still returns a
network directly.

To keep row indices fixed instead, `keep_island(&buses)` works in place: it
de-energizes every other bus not already typed isolated and returns those
buses, so the equipment outside the island stays in its table, out of
service. `de_energize_buses(&buses)` is the step it builds on, the same one
`assign_island_references` applies to an island with no source; it also takes
the generators at those buses out of service.

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
  Three winding transformers lose only windings on those buses; the whole
  transformer goes out when fewer than two live windings remain.
  Supply counts in-service generators only, so an island that only an HVDC
  line feeds is de-energized too, and the line with it. It gets its own code,
  `CANONICALIZE.ISLAND.HVDC_FED_DE_ENERGIZED`, which names the lines and the
  power they schedule into the island; model that infeed as a generator at
  the converter bus to keep the island.

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

### PSS/E impedance correction before topology reduction

`BalancedNetwork::apply_impedance_correction()` applies retained PSS/E
transformer correction tables at the present tap or phase shift. Call it
before `merge_buses` or closed-switch normalization: a bypassed transformer
becomes a shunt, whose admittance must include the correction. Reapply after
editing tap or shift values while the transformer still exists; the nominal
cache prevents repeated scaling.

The original table range selects ratio or angle. Ratio tables use bus-base
per-unit winding ratios regardless of the RAW `CW` setting. A branch table
is mapped to its effective t1/t2 tap coordinates. Three-winding `ZCOD=0`
scales star impedances; `ZCOD=1` scales Z12/Z23/Z31 at the one nonfixed
control winding, even when automatic regulation is disabled. Ambiguous RAW
`ZCOD=1` input is refused; an edited in-memory transformer with no unique
controlling winding is skipped by correction application.

Fresh RAW35 output states nominal impedances beside the tables and preserves
their interpolation and angle/ratio classification. When winding 2 cannot
be set to one without changing a ratio table's meaning, the writer chooses
an equivalent nonunit ratio. RAW33/34 cannot state `ZCOD=1`; output freezes
its present corrected pairwise impedances and reports the lost dependence
on tap position. RAWX reads and writes the same tables as its `impcor`
table, one `itable, tap, refact, imfact` row per point, with the RAW 35
rules.
