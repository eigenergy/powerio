# PowerIO

<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/60e0126c/docs/src/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO 0.10 is the public beta of the 1.0 API. It parses power system data
into typed values, converts supported formats, and builds sparse matrices and
graph data.

A parse returns a `PioModule`: one typed value plus its sources, diagnostics,
source map, and history. The value can be a network, calculation instance,
solution, time series, or scenario set.

## Install

```sh
cargo add powerio
pip install powerio
julia -e 'using Pkg; Pkg.add("PowerIO")'
```

Optional Python dependencies are installed separately:

```sh
pip install 'powerio[matrix]'
pip install 'powerio[graph]'
pip install 'powerio[all]'
```

Install the command line program with `cargo install powerio-cli`.

## Parse and write

Rust:

```rust,ignore
use powerio::{BalancedNetwork, Destination, PioModule, Source};

let module = powerio::parse(Source::open("case9.m")?)?;
let module: PioModule<BalancedNetwork> = powerio::try_into_typed(module)?;
let module = module.map_value(powerio::PioValue::from);
powerio::write_module_as(&module, "matpower", Destination::path("copy.m"))?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Python detects the kind without a type argument:

```python
import powerio

module = powerio.parse("case9.m")
module.kind                         # "balanced_network"
network = module.value
module.diagnostics()               # native diagnostic records
network.write_file("copy.m", "matpower")  # byte exact same format write
```

Pass `value_type=powerio.BalancedNetwork` only when the caller wants to assert
the expected kind while parsing.

Julia uses multiple dispatch on the typed value:

```julia
using PowerIO

module_ = parse_file("case9.m")     # PioModule{BalancedNetwork}
network = module_.value
n_buses(network)                    # 9
diagnostics(module_)
write_file(module_, "copy.m")
```

The command line interface uses the same readers and writers:

```sh
powerio convert case9.m --to psse -o case9.raw
powerio module case9.m -o case9.pio.json
powerio verify case9.m --kind bdoubleprime
powerio sensitivities case9.m -o out
```

An unchanged module writes its source format byte for byte. Cross format
conversion keeps what the destination can represent and returns a diagnostic
for each loss. `.pio.json` version 1 stores a module for exchange between
PowerIO consumers; it does not replace domain formats such as MATPOWER,
PSS/E, or OpenDSS.

## Supported values and formats

Balanced network formats:

- MATPOWER `.m`
- PSS/E `.raw` revisions 33, 34, and 35
- PowerWorld `.aux`; `.pwb` is read only and `.pwd` uses the display API
- GE PSLF `.epc`
- PowerModels JSON
- Egret JSON
- pandapower JSON
- PyPSA CSV directories
- Surge JSON

Multiconductor distribution formats:

- OpenDSS `.dss`
- PowerModelsDistribution engineering JSON
- IEEE BMOPF JSON

Calculation and dataset inputs:

- DOE GO Challenge 3 JSON produces `AcScucInstance`
- BMOPF JSON produces `McAcOpfInstance`
- DeepMind OPFData JSON produces `AcOpfSolution`
- GridFM Parquet produces `ScenarioSet<BalancedNetwork>`
- Supported PyPSA and Egret profiles can produce typed time series

[Formats and Fidelity](docs/src/format-fidelity.md) lists
the supported profile and write behavior for every format.

## Package structure

```text
powerio-core     Source, PioModule, diagnostics, time series, and output types
powerio-tx       balanced transmission model, parsers, and writers
powerio-dist     multiconductor distribution model and format support
powerio-prob     operating points, calculation instances, and solutions
powerio-matrix   sparse matrices, sensitivities, and graph data
powerio          entry facade, dynamic values, dispatch, and .pio.json
powerio-capi     C ABI for C, C++, Julia, and other bindings
powerio-py       native extension for the Python package
powerio-cli      command line interface and terminal interface
```

`powerio-tx` and `powerio-dist` are independent and share `powerio-core`.
`powerio-prob` is matrix free. `powerio-matrix` depends on the component crates,
and the `powerio` facade provides the combined entry point without defining a
preferred universal power system format.

## Documentation

- [Guide](docs/src/README.md)
- [Core concepts](docs/src/concepts.md)
- [PowerIO intermediate representations](docs/src/architecture.md)
- [Matrices and signs](docs/src/matrices.md)
- [Rust, Python, Julia, and C](docs/src/languages.md)
- [Python API](docs/src/python.md)
- [C ABI](docs/src/capi.md)
- [PowerIO.jl](https://eigenergy.github.io/PowerIO.jl)

Migration notes, retired names, ABI history, internal crate rules, performance
evidence, and release checks are under Developer Guides.

## License

PowerIO is available under either the Apache License 2.0 or the MIT License.
