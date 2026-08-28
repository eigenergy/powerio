# PowerIO guide

PowerIO 0.10 is the public beta of the 1.0 API. API corrections may land before 1.0.0 as downstream integrations exercise the new design.

PowerIO parses power system data into typed values, converts between supported formats, and builds sparse matrices and graph data for solvers and analysis code. One call reads any supported source:

```julia
using PowerIO
case = parse_file("case9.m")       # PioModule{BalancedNetwork}
feeder = parse_file("feeder.dss")  # PioModule{MulticonductorNetwork}
```

```rust,ignore
let module = powerio::parse(Source::open("case9.m")?)?;   // PioModule<PioValue>
let case: PioModule<BalancedNetwork> = powerio::try_into_typed(module)?;
```

```python
import powerio
case = powerio.parse("case9.m")    # PioModule, kind "balanced_network"
```

The result is a module: one typed value plus the records that explain it, the retained source bytes, the reader's findings, and the operations that produced it. What the value is depends on what the source declares. A MATPOWER case is a balanced network. An OpenDSS feeder is a multiconductor network. A DOE GO Challenge 3 file defines a unit commitment calculation, so it parses to that calculation's input. A DeepMind OPFData file records a solved AC OPF, so it parses to a solution. A PyPSA folder with a snapshot axis parses to a time series, and a GridFM Parquet dataset to a scenario set. [Core Concepts](concepts.md) defines the families.

Three rules hold everywhere:

- writing an unchanged module back to its own format reproduces the source bytes exactly;
- converting to another format keeps everything the target can represent and reports each loss as a coded finding;
- moving between value families is an explicit, recorded operation, never a side effect.

Supported sources: MATPOWER, PSS/E revisions 33 through 35, PowerWorld AUX and PWB, PSLF EPC, PowerModels JSON, Egret JSON, pandapower JSON, PyPSA CSV folders, Surge JSON, DOE GO Challenge 3 JSON, DeepMind OPFData JSON, GridFM Parquet datasets, OpenDSS, PowerModelsDistribution engineering JSON, BMOPF JSON, and the stored `.pio.json` document. [Formats and Fidelity](format-fidelity.md) states each format's supported profile and write support.

The same operations run from Rust, Python, Julia, C, the `powerio` command line tool, and the MCP server, with the same value kinds, format names, diagnostic codes, and conventions. [Rust, Python, Julia, and C](languages.md) maps the operations across languages; [0.10 Beta Scope and Known Limits](beta-scope.md) states what this beta includes and what waits for 1.0.
