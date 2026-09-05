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
