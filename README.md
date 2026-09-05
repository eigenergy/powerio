# PowerIO

[![Rust](https://github.com/eigenergy/powerio/actions/workflows/rust.yml/badge.svg?branch=main)](https://github.com/eigenergy/powerio/actions/workflows/rust.yml)
[![Python](https://github.com/eigenergy/powerio/actions/workflows/python.yml/badge.svg?branch=main)](https://github.com/eigenergy/powerio/actions/workflows/python.yml)
[![Julia binding](https://github.com/eigenergy/powerio/actions/workflows/julia-binding.yml/badge.svg?branch=main)](https://github.com/eigenergy/powerio/actions/workflows/julia-binding.yml)
[![crates.io](https://img.shields.io/crates/v/powerio)](https://crates.io/crates/powerio)
[![docs.rs](https://docs.rs/powerio/badge.svg)](https://docs.rs/powerio)
[![PyPI](https://img.shields.io/pypi/v/powerio)](https://pypi.org/project/powerio/)
[![MSRV 1.88](https://img.shields.io/badge/MSRV-1.88-blue.svg)](Cargo.toml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE-MIT)

<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/60e0126c/docs/src/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO reads power system data into typed values, writes those values back
out in the formats other tools read, and builds the sparse matrices and graph
data that solvers consume. The core is Rust, and the Python package, the
Julia package [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl), and the
C ABI expose the same operations under the same names.

A parse returns a `PioModule`, which is one typed value together with its
sources, diagnostics, source map, and history. Depending on what the source
declares, the value is a network, a calculation instance, a solution, a time
series, a scenario set, or a geographic layer.

## Install

```sh
cargo add powerio                 # parsing, emission, PowerIO IR
cargo add powerio -F matrix       # and sparse matrices, sensitivities, graph data
pip install powerio               # pip install 'powerio[all]' adds SciPy, NetworkX, Polars
julia -e 'using Pkg; Pkg.add("PowerIO")'
cargo install powerio-cli         # the powerio command
```

## Parse and emit

The examples use [case9.m](tests/data/case9.m), the nine-bus MATPOWER case
included in this repository. Save it in your working directory first, or
replace `"case9.m"` with the path to your own case.

Rust:

```rust,ignore
use powerio::{PioValue, emit, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let module = parse("case9.m")?;
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("expected a balanced network");
    };
    assert_eq!(network.buses().len(), 9);
    emit(&module, "matpower", "copy.m")?;   // the source bytes, unchanged
    let result = emit(&module, "psse", "case9.raw")?;
    for finding in result.diagnostics() {
        eprintln!("{}", finding.code());    // what PSS/E cannot carry
    }
    Ok(())
}
```

Python:

```python
import powerio

module = powerio.parse("case9.m")
network = module.value
assert isinstance(network, powerio.BalancedNetwork)
module.diagnostics                          # the reader's findings
powerio.emit(module, "matpower", "copy.m")  # the source bytes, unchanged
result = powerio.emit(module, "psse")       # text in memory plus findings
```

Julia:

```julia
using PowerIO

module_ = parse("case9.m")            # PioModule{BalancedNetwork}
net = module_.value
length(net.buses)                     # 9
module_.diagnostics
emit(module_, "matpower", "copy.m")
```

Command line:

```sh
powerio convert case9.m --to psse -o case9.raw
powerio serialize case9.m -o case9.pio.json
powerio verify case9.m --kind bdoubleprime
powerio sensitivities case9.m -o out
```

If you write an unchanged module back to its own format, you get the source
back byte for byte. If you write it to another format, PowerIO keeps what
that format can represent and reports each loss as a diagnostic with a stable
code, so you can see what the conversion dropped. `serialize` and
`deserialize` write and read PowerIO IR, the JSON document that stores a
complete module for another PowerIO consumer to read back. It preserves the
typed data and its history; retained source bytes stay in the running process.
After deserialization, `emit` writes fresh output from the stored value.

Upgrading from 0.10? Start with the
[migration guide](https://eigenergy.github.io/powerio/guide/migration-0.11.html).
PowerIO 0.11 uses PowerIO IR generation 2 and C ABI 7; these version numbers
describe separate interfaces.

## Formats

| Format | Token | Read | Write |
|---|---|---|---|
| MATPOWER `.m` | `matpower` | yes | yes |
| PSS/E RAW | `psse`, `psse34`, `psse35` | revisions 32 to 35 | 33, 34, 35 |
| PSS/E RAWX | `psse-rawx` | revision 35 | 35 |
| PowSybl XIIDM | `xiidm` | 1.0 to 1.17 | 1.17 |
| PowSybl JIIDM | `jiidm` | 1.0 to 1.17 | 1.17 |
| CIM CGMES profile set | `cgmes` | 2.4.15, 3.0 | 3.0 |
| ENTSO-E UCTE-DEF `.uct` | `ucte` | 2003.09.01, 2007.05.01 | 2007.05.01 |
| PowerWorld `.aux` | `powerworld` | yes | yes |
| PowerWorld `.pwb` | `pwb` | yes | no |
| PowerWorld `.pwd` display | `powerworld-pwd` | as a geographic layer | no |
| GE PSLF `.epc` | `pslf` | yes | yes |
| IEEE Common Data Format | `ieee-cdf` | yes | no |
| PowerModels JSON | `powermodels-json` | yes | yes |
| Egret JSON | `egret-json` | yes | yes |
| pandapower JSON | `pandapower-json` | yes | yes |
| PyPSA CSV directory | `pypsa-csv` | yes | yes |
| Surge JSON | `surge-json` | yes | yes |
| GridFM Parquet directory | `gridfm` | yes | yes |
| DOE GO Challenge 3 JSON | `goc3-json` | problem, or problem with solution | a complete solution |
| DeepMind OPFData JSON | `opfdata-json` | yes | no |
| OpenDSS `.dss` | `dss` | yes | yes |
| PowerModelsDistribution engineering JSON | `pmd-json` | yes | yes |
| BMOPF JSON | `bmopf-json` | 0.1.0, 0.2.0 | 0.2.0 |
| Geographic layer `.geo.json` | `geo-json` | yes | yes |

The balanced transmission formats parse to `BalancedNetwork`, and OpenDSS,
PMD, and BMOPF parse to `MulticonductorNetwork`. A GO Challenge 3 problem
parses to `AcScucInstance`, a problem with its solution to `AcScucSolution`,
an OPFData file to `AcOpfSolution`, a GridFM dataset to
`ScenarioSet<BalancedNetwork>`, and a PyPSA directory with several snapshots
to a `TimeSeries`. The
[format guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html)
says what each reader keeps, what each writer reports, and how each is
checked against its reference implementation.

## Packages

Start with `powerio` in an application. The component crates let libraries
depend on just the layer they use.

| Crate | Provides |
|---|---|
| `powerio` | `PioValue`, `parse`, `emit`, `serialize`, `deserialize` |
| `powerio-core` | `Source`, `PioModule`, diagnostics, collections, destinations |
| `powerio-tx` | `BalancedNetwork` and balanced format readers and writers |
| `powerio-dist` | `MulticonductorNetwork` and OpenDSS, PMD, BMOPF converters |
| `powerio-prob` | Operating points, updates, calculation instances and solutions |
| `powerio-matrix` | Sparse matrices, sensitivities, DC OPF bundles and graph data |
| `powerio-cli` | The `powerio` command and its terminal interface |
| `powerio-py` | The native extension used by the Python package |
| `powerio-capi` | C ABI 7, used by Julia and other FFI bindings |

`powerio-tx` and `powerio-dist` share `powerio-core` and nothing else, so a
distribution consumer pulls in no transmission code. `powerio-prob` is matrix
free. The `powerio` facade re-exports the component crates, with
`powerio-matrix` behind its `matrix` feature.

## Documentation

- [Guide](https://eigenergy.github.io/powerio/guide/)
- [Core concepts](https://eigenergy.github.io/powerio/guide/concepts.html)
- [Formats and fidelity](https://eigenergy.github.io/powerio/guide/format-fidelity.html)
- [Matrices and graphs](https://eigenergy.github.io/powerio/guide/matrices.html)
- [Rust, Python, Julia, and C](https://eigenergy.github.io/powerio/guide/languages.html)
- [Rust API reference](https://eigenergy.github.io/powerio/powerio/)
- [PowerIO.jl](https://eigenergy.github.io/PowerIO.jl)

## License

Apache License 2.0 or MIT License, at your option.
