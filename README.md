# PowerIO

<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/60e0126c/docs/src/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO 0.11 incorporates the API corrections found while exercising the 0.10
beta with external solver consumers. It is the stabilization line for the
candidate 1.0 API: 0.11.x is reserved for compatible fixes, performance
improvements, and additive work. PowerIO parses power system data into typed
values, emits supported formats, and builds sparse matrices and graph data.

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

## Parse and emit

Rust:

```rust,ignore
use powerio::PioValue;

let module = powerio::parse("case9.m")?;
let PioValue::BalancedNetwork(network) = &module.value else {
    panic!("expected a balanced network");
};
assert_eq!(network.buses().len(), 9);
powerio::emit(&module, "matpower", "copy.m")?;
# Ok::<(), Box<dyn std::error::Error>>(())
```

Python uses the value's concrete type:

```python
import powerio

module = powerio.parse("case9.m")
network = module.value
assert isinstance(network, powerio.BalancedNetwork)
module.diagnostics                 # native diagnostic records
powerio.emit(module, "matpower", "copy.m")  # byte exact same format emission
```

Pass a file object for text already in memory and a bytes-like object for
binary data. A Python `str` names a path.

Julia uses multiple dispatch on the typed value:

```julia
using PowerIO

module_ = parse("case9.m")          # PioModule{BalancedNetwork}
network = module_.value
length(network.buses)               # 9
module_.diagnostics
emit(module_, "matpower", "copy.m")
```

The command line interface uses the same parsers and emitters:

```sh
powerio convert case9.m --to psse -o case9.raw
powerio serialize case9.m -o case9.pio.json
powerio verify case9.m --kind bdoubleprime
powerio sensitivities case9.m -o out
```

An unchanged module emits its source format byte for byte. Cross format
conversion keeps what the destination can represent and returns a diagnostic
for each loss. `serialize` writes PowerIO IR and `deserialize` reads it. The IR
does not replace grid exchange formats such as MATPOWER, PSS/E, or OpenDSS.

## Supported values and formats

Balanced network formats:

- MATPOWER `.m`
- PSS/E `.raw` revisions 32 through 35; fresh output uses 33, 34, or 35
- PSS/E RAWX JSON revision 35
- PowSybl XIIDM XML 1.12 through 1.17; fresh output uses 1.17
- CIM CGMES 2.4.15 and 3.0 profile sets; fresh output uses CGMES 3.0
- ENTSO-E UCTE-DEF `.uct` revisions 2003.09.01 and 2007.05.01; fresh output uses 2007.05.01
- PowerWorld `.aux`; `.pwb` is read only and `.pwd` uses the display API
- GE PSLF `.epc`
- IEEE Common Data Format (`ieee-cdf`), read only
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

- a DOE GO Challenge 3 problem data file produces `AcScucInstance`; the problem together with its matching solution data produces `AcScucSolution`, and complete solutions emit the official solution JSON shape
- DeepMind OPFData JSON produces `AcOpfSolution`
- GridFM Parquet produces `ScenarioSet<BalancedNetwork>`
- Supported PyPSA and Egret profiles can produce typed time series

[Formats and Fidelity](docs/src/format-fidelity.md) lists
the supported profile and write behavior for every format.

## Package structure

```text
powerio-core     Source, PioModule, diagnostics, time series, and output types
powerio-tx       balanced transmission model, parsers, and emitters
powerio-dist     multiconductor distribution model and format support
powerio-prob     operating points, calculation instances, and solutions
powerio-matrix   sparse matrices, sensitivities, and graph data
powerio          entry facade, dynamic values, dispatch, and PowerIO IR
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
