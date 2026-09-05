# PowerIO guide

PowerIO reads power system data into typed values, writes those values in the
formats other tools read, and builds the sparse matrices and graph data that
solvers consume. The Rust crates are the implementation; the Python package,
[PowerIO.jl](https://eigenergy.github.io/PowerIO.jl), and the C ABI expose
the same operations under the same names.

```rust,ignore
use powerio::{PioValue, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = parse("case9.m")?;         // PioModule<PioValue>
    match module.value() {
        PioValue::BalancedNetwork(network) => {
            println!("{} buses", network.buses().len());
        }
        other => println!("{}", other.type_name()),
    }
    Ok(())
}
```

```python
import powerio
module = powerio.parse("case9.m")                 # PioModule[BalancedNetwork]
```

```julia
using PowerIO
module_ = parse("case9.m")                        # PioModule{BalancedNetwork}
feeder = parse("IEEE13Nodeckt.dss")               # PioModule{MulticonductorNetwork}
```

A parse returns a module: one typed value together with the records that
explain it, which are the source descriptions, the source map, the reader's
diagnostics, the derivation history, and, while the process runs, the source
bytes themselves. The value depends on what the source declares:

| Source | Value |
|---|---|
| MATPOWER, PSS/E, XIIDM, CGMES, UCTE-DEF, and the other balanced formats | `BalancedNetwork` |
| OpenDSS, PowerModelsDistribution JSON, BMOPF JSON | `MulticonductorNetwork` |
| a PyPSA directory whose inputs vary by snapshot | `TimeSeries<BalancedNetwork>` |
| a GridFM Parquet dataset | `ScenarioSet<BalancedNetwork>` |
| a DOE GO Challenge 3 problem file | `AcScucInstance` |
| that problem file beside its solution file | `AcScucSolution` |
| a DeepMind OPFData file | `AcOpfSolution` |
| a geographic layer document or a PowerWorld `.pwd` display | `GeoLayer` |

Four operations move data. `parse` reads a grid exchange format into a
module. `emit` writes a module in a grid exchange format. `serialize` and
`deserialize` move PowerIO IR, the JSON document that carries a complete
module between PowerIO consumers. PowerIO IR is not a grid exchange format,
and `parse` does not accept it.

Three rules hold everywhere:

- writing an unchanged module back to its own format reproduces the source
  bytes exactly;
- writing another format keeps what that format can state and reports each
  loss as a diagnostic with a stable code;
- moving between value families is an explicit operation that records itself
  in the module history, never a side effect.

[Getting started](getting-started.md) installs PowerIO and runs a first
conversion. [Core concepts](concepts.md) defines the module, the value
families, diagnostics, and sources. [Formats and fidelity](format-fidelity.md)
states what each reader keeps and each writer reports. [Rust, Python, Julia,
and C](languages.md) maps every operation across the four languages.
