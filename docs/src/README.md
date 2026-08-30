# PowerIO guide

PowerIO 0.10 established the public beta of the 1.0 API. The 1.0 candidate
applies the corrections found while building external solver consumers.

PowerIO parses power system data into typed values, transforms those values, emits supported formats, and builds sparse matrices and graph data for solvers and analysis code. One call reads any supported file or case directory:

```julia
using PowerIO
case = parse_file("case9.m")               # PioModule{BalancedNetwork}
feeder = parse_file("IEEE13Nodeckt.dss")   # PioModule{MulticonductorNetwork}
```

```rust,ignore
let module = powerio::parse_file("case9.m")?;   // PioModule<PioValue>
match module.value() {
    powerio::PioValue::BalancedNetwork(network) => {
        println!("{} buses", network.buses().len());
    }
    other => println!("{}", other.kind().as_str()),
}
```

```python
import powerio
case = powerio.parse_file("case9.m")    # PioModule, kind "balanced_network"
```

The result is a module: one typed value plus source descriptions, a source map, diagnostics, history, and retained source bytes. What the value is depends on what the source declares. A MATPOWER case is a balanced network. An OpenDSS feeder is a multiconductor network. A DOE GO Challenge 3 file defines a unit commitment calculation, so it parses to that calculation's input. A DeepMind OPFData file records a solved AC OPF, so it parses to a solution. A PyPSA folder with a snapshot axis parses to a time series, and a GridFM Parquet dataset to a scenario set. [Core Concepts](concepts.md) defines the families.

Three rules hold everywhere:

- emitting an unchanged module back to its own format reproduces the source bytes exactly;
- emitting another format keeps everything the target can represent and reports each loss as a coded diagnostic;
- moving between value families is an explicit, recorded operation, never a side effect.

Supported sources: MATPOWER, PSS/E revisions 33 through 35, PowerWorld AUX and PWB, PSLF EPC, PowerModels JSON, Egret JSON, pandapower JSON, PyPSA CSV folders, Surge JSON, DOE GO Challenge 3 JSON, DeepMind OPFData JSON, GridFM Parquet datasets, OpenDSS, PowerModelsDistribution engineering JSON, BMOPF JSON, and the stored `.pio.json` document. [Formats and Fidelity](format-fidelity.md) states each format's supported profile and write support.

Operations exposed on more than one surface use the same value kinds, format
names, diagnostic codes, signs, and units. [Rust, Python, Julia, and C](languages.md)
maps actual coverage; [1.0 Scope and Known Limits](scope-v1.md) lists
surface specific limits.
