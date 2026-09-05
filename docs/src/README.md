# PowerIO guide

PowerIO reads power system data into typed values, writes those values in the
formats other tools read, and builds the sparse matrices and graph data that
solvers consume. The Rust crates are the implementation, and the Python
package, [PowerIO.jl](https://eigenergy.github.io/PowerIO.jl), and the C ABI
expose the same operations under the same names.

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

A parse returns a module, which is one typed value together with the data
that explains it: the source descriptions, the source map, the reader's
diagnostics, the derivation history, and, while the process runs, the source
bytes themselves. Which value you get depends on what the source declares:

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

`parse` reads a grid exchange format into a module, and `emit` writes a
module out in one. The other two operations, `serialize` and `deserialize`,
write and read PowerIO IR, the JSON document that stores a complete module for
another PowerIO consumer to read back. PowerIO IR is not a grid exchange
format, so `parse` does not accept it; `deserialize` does.

Whichever format you use, writing an unchanged module back to its own format
reproduces the source bytes byte for byte, and writing to another format
keeps what that format can represent and reports each loss as a diagnostic
with a stable code. Converting from one kind of value to another is an
explicit operation that adds an entry to the module history; nothing converts
as a side effect.

To install PowerIO and run a first conversion, start with
[Getting started](getting-started.md). [Core concepts](concepts.md) defines
the module, the value types, diagnostics, and sources.
[Formats and fidelity](format-fidelity.md) says what each reader keeps and
each writer reports, and [Rust, Python, Julia, and C](languages.md) shows
each operation in all four languages.
