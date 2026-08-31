# Time Series and Scenarios

Some sources declare more than one state of a system. PowerIO keeps the declared structure instead of flattening it: an ordered sequence of complete values is a time series, a set of named alternatives is a scenario set, and the element type states what varies.

| Source | Parses to |
|---|---|
| PyPSA CSV folder, snapshot axis with varying inputs | `TimeSeries<BalancedNetwork>` |
| PyPSA CSV folder, fixed network with complete state output per snapshot | `TimeSeries<OperatingPoint<BalancedNetwork>>` |
| Egret JSON with `system.time_keys` in the scalar profile | `TimeSeries<BalancedNetwork>` |
| GridFM Parquet dataset | `ScenarioSet<BalancedNetwork>` |

A time point keeps the source's exact label and, when the source states one, an interval duration; PowerIO imposes no calendar meaning on labels. Scenario identifiers are the source's own strings, looked up by identifier rather than position. Entries share the base network's data: parsing a thousand scenario dataset does not build a thousand networks.

An operating point is a complete assignment of the instantaneous electrical variables over a network: voltages, injections, and the equipment settings that change the equations (switch status, taps, capacitor steps). A varying input like a load schedule or a time varying limit is not an operating point; a source that varies inputs parses as a series of networks.

```julia
using PowerIO
dataset = parse_file("gridfm_case14/")     # PioModule{ScenarioSet{BalancedNetwork}}
keys(dataset)                              # the scenario identifiers
one = dataset["7"]                         # PioModule{BalancedNetwork}, shared data

series = parse_file("pypsa_folder/")       # PioModule{TimeSeries{BalancedNetwork}}
length(series)                             # number of declared time points
first_hour = series[1]                     # one based, like every Julia axis
```

Selection returns an independent module over the existing typed entry; nothing reparses and no numerical table is copied. Emitting the selection to a file or a stored document uses the same operation as every other module.

Data outside a source's supported profile (PyPSA investment periods, stochastic scenarios, unit commitment coupling; Egret reserve and contingency data) stays in the retained source, is reported as uninterpreted, and survives a same format emission. [Formats and Fidelity](format-fidelity.md) states each profile's boundary.
