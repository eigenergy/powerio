# Time series and scenarios

Some sources declare more than one value, and PowerIO keeps the structure
they declare. An ordered sequence of values is a time series, a set of named
alternatives is a scenario set, and the element type says what varies from
one entry to the next.

| Source | Parses to |
|---|---|
| PyPSA CSV directory, snapshot axis with varying inputs | `TimeSeries<BalancedNetwork>` |
| PyPSA CSV directory, fixed network with electrical assignments per snapshot | `TimeSeries<OperatingPoint<BalancedNetwork>>` |
| Egret JSON with `system.time_keys` in the scalar profile | `TimeSeries<BalancedNetwork>` |
| GridFM Parquet dataset (the `gridfm` feature) | `ScenarioSet<BalancedNetwork>` |

A time point keeps the source's exact label and, when the source gives one,
an interval duration; PowerIO imposes no calendar meaning on labels. Scenario
identifiers are the source's own strings, and you look scenarios up by
identifier rather than by position. An operating point entry refers to its
shared base network instead of copying the network tables.
[Core concepts](concepts.md) says what an operating point can and cannot
change.

```julia
using PowerIO
dataset = parse("gridfm_case14/")          # PioModule{ScenarioSet{BalancedNetwork}}
scenarios = dataset.value
keys(scenarios)                            # the scenario identifiers
one = scenarios["7"]                       # BalancedNetwork

series_module = parse("pypsa_folder/")     # PioModule{TimeSeries{BalancedNetwork}}
series = series_module.value
length(series)                             # the number of declared time points
first_hour = series[1]                     # one based, like every Julia axis
```

Indexing returns the contained entry or a view rooted in the owning module;
nothing reparses and no complete network is copied. If an entry needs
diagnostics, history, emission, or serialization of its own, wrap it in a
module.

Only the facade reads the Egret time series. `powerio::parse` returns the
series, while the component crate's `powerio_tx::parse` reads only the
static profile and refuses a document with `system.time_keys`.

Data outside a source's profile, such as PyPSA investment periods,
stochastic scenarios, and unit commitment coupling, or Egret reserve and
contingency data, stays in the retained source, is reported as
uninterpreted, and survives a same format emission.
[Formats and fidelity](format-fidelity.md) says where each profile ends.

## Egret electrical time series

Egret branch ratings use `null` (or an absent scalar field) for unlimited
flow. Canonical output writes, for example, `[null, 150.0]` when PowerIO's
`rate_a` values are `[0.0, 150.0]`. This applies to long-term, short-term,
and emergency ratings. A native Egret numeric zero imposes a zero thermal
limit, which PowerIO's branch rating fields cannot represent because they
reserve zero for unlimited flow. PowerIO 0.11 still reads such a zero, scalar
or at any time point, as unlimited, as earlier releases did. Refusing it is
tracked for 0.12.

PowerIO 0.11.10 and earlier reject `null` inside rating time series. Upgrade
those readers before reimporting corrected canonical output. Native Egret
accepts these unlimited points.

Earlier PowerIO output could put numeric zero in mixed rating series.
Regenerate those files from the original model. Change zero to `null` in an
existing Egret file only after confirming that the source intended unlimited
flow. A PowerIO IR document made from a native Egret zero holds unlimited and
has lost that distinction; regenerate it from its original Egret input if the
zero limit matters. The PowerIO IR layout and branch convention are unchanged.

Egret `system.time_keys` and per-element `data_type: "time_series"` values
parse into `TimeSeries<BalancedNetwork>`. Labels must be unique. A stated
`time_period_length_minutes` must be finite and positive and is retained in
each time point. Canonical Egret emission survives IR serialization and also
accepts balanced operating-point series, including the supported PyPSA profile.
It writes native branch results `pf`, `qf`, `pt`, `qt` in MW/MVAr, with each end's
injection sign. Partial DC-only results remain partial; missing reactive values
are not invented.

The writer supports changing loads, dispatch and bounds, voltage states and
bounds, shunts, branch ratings/taps/shifts/status, and complete branch flows.
Topology, element identities and unsupported attributes must remain static, and
all points must share one optional duration. Unsupported variation is refused,
including a single-point series. This is an electrical snapshot profile, not
Egret's unit-commitment formulation: commitment, ramping, reserve and other
intertemporal optimization semantics are not modeled. Selecting a snapshot can
feed any compatible static writer; arbitrary temporal export to every format
is not promised. Retained source bytes still provide exact same-format replay.
