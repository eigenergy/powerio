# Time series and scenarios

Some sources declare more than one value. PowerIO keeps the declared
structure: an ordered sequence of values is a time series, a set of named
alternatives is a scenario set, and the element type says what varies.

| Source | Parses to |
|---|---|
| PyPSA CSV directory, snapshot axis with varying inputs | `TimeSeries<BalancedNetwork>` |
| PyPSA CSV directory, fixed network with electrical assignments per snapshot | `TimeSeries<OperatingPoint<BalancedNetwork>>` |
| Egret JSON with `system.time_keys` in the scalar profile | `TimeSeries<BalancedNetwork>` |
| GridFM Parquet dataset (the `gridfm` feature) | `ScenarioSet<BalancedNetwork>` |

A time point keeps the source's exact label and, when the source states one,
an interval duration. PowerIO imposes no calendar meaning on labels. Scenario
identifiers are the source's own strings, looked up by identifier rather than
position. An operating point entry refers to its shared base network instead
of copying the network tables. [Core concepts](concepts.md) states what an
operating point can and cannot change.

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

Indexing returns the contained entry or a view rooted in the owning module.
Nothing reparses and no complete network is copied. Wrap a value in a module
when it needs diagnostics, history, emission, or serialization of its own.

The Egret time series is a facade reading: `powerio::parse` returns the
series, while the component crate's `powerio_tx::parse` reads only the
static profile and refuses a document with `system.time_keys`.

Data outside a source's profile, such as PyPSA investment periods,
stochastic scenarios, and unit commitment coupling, or Egret reserve and
contingency data, stays in the retained source, is reported as
uninterpreted, and survives a same format emission.
[Formats and fidelity](format-fidelity.md) states each profile's boundary.
