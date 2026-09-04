# Time Series and Scenarios

Some sources declare more than one value. PowerIO keeps the declared structure instead of flattening it: an ordered sequence of values is a time series, a set of named alternatives is a scenario set, and the element type says what varies.

| Source | Parses to |
|---|---|
| PyPSA CSV folder, snapshot axis with varying inputs | `TimeSeries<BalancedNetwork>` |
| PyPSA CSV folder, fixed network with electrical assignments per snapshot | `TimeSeries<OperatingPoint<BalancedNetwork>>` |
| Egret JSON with `system.time_keys` in the scalar profile | `TimeSeries<BalancedNetwork>` |
| GridFM Parquet dataset | `ScenarioSet<BalancedNetwork>` |

A time point keeps the source's exact label and, when the source states one, an interval duration; PowerIO imposes no calendar meaning on labels. Scenario identifiers are the source's own strings, looked up by identifier rather than position. An operating point entry roots its shared base network instead of copying the network tables.

An operating point is a possibly partial alternate electrical assignment over fixed equipment identities: demand, setpoints, dispatch, voltages, injections, service status, switch positions, transformer taps, and phase shifts. Missing quantities resolve to the network's source/default assignment. A time varying physical parameter, rating, cost, or equipment inventory is not an operating point; a source that varies those values contains networks or calculation instances.

```julia
using PowerIO
dataset = parse("gridfm_case14/")          # PioModule{ScenarioSet{BalancedNetwork}}
scenarios = dataset.value
keys(scenarios)                            # the scenario identifiers
one = scenarios["7"]                       # BalancedNetwork

series_module = parse("pypsa_folder/")     # PioModule{TimeSeries{BalancedNetwork}}
series = series_module.value
length(series)                             # number of declared time points
first_hour = series[1]                     # one based, like every Julia axis
```

Indexing returns the contained typed entry or an owner rooted view; nothing reparses and no complete network is copied. Wrap a value in a module when it needs module diagnostics, derivation history, emission, or serialization.

Data outside a source's supported profile (PyPSA investment periods, stochastic scenarios, unit commitment coupling; Egret reserve and contingency data) stays in the retained source, is reported as uninterpreted, and survives a same format emission. [Formats and Fidelity](format-fidelity.md) states each profile's boundary.
