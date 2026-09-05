# PowerIO

<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/60e0126c/docs/src/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO reads power system data into typed values, writes those values back
out in the formats other tools read, and builds the sparse matrices and graph
data that solvers consume. The core is Rust. The Python package, the Julia
package [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl), and the C ABI
expose the same operations under the same names.

A parse returns a `PioModule`: one typed value together with its sources,
diagnostics, source map, and history. The value is a network, a calculation
instance, a solution, a time series, a scenario set, or a geographic layer,
depending on what the source declares.

## Install

```sh
cargo add powerio                 # parsing, emission, PowerIO IR
cargo add powerio -F matrix       # and sparse matrices, sensitivities, graph data
pip install powerio               # pip install 'powerio[all]' adds SciPy, NetworkX, Polars
julia -e 'using Pkg; Pkg.add("PowerIO")'
cargo install powerio-cli         # the powerio command
```

## Parse and emit

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

Writing an unchanged module back to its own format reproduces the source byte
for byte. Writing another format keeps what that format can state and reports
each loss as a diagnostic with a stable code. `serialize` and `deserialize`
move PowerIO IR, the JSON document that carries a complete module between
PowerIO consumers; it is not a grid exchange format.

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

Balanced transmission formats parse to `BalancedNetwork`; OpenDSS, PMD, and
BMOPF parse to `MulticonductorNetwork`. A GO Challenge 3 problem parses to
`AcScucInstance`, a problem with its solution to `AcScucSolution`, an OPFData
file to `AcOpfSolution`, a GridFM dataset to `ScenarioSet<BalancedNetwork>`,
and a PyPSA directory with several snapshots to a `TimeSeries`. The
[format guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html)
states what each reader keeps, what each writer reports, and how each is
checked against its reference implementation.

## Packages

```text
powerio-core     Source, PioModule, diagnostics, time series, scenario sets, destinations
powerio-tx       BalancedNetwork and the balanced format readers and writers
powerio-dist     MulticonductorNetwork and the OpenDSS, PMD, and BMOPF converters
powerio-prob     operating points, updates, calculation instances, and solutions
powerio-matrix   sparse matrices, sensitivities, the DC OPF bundle, and graph data
powerio          the facade: PioValue, parse, emit, serialize, deserialize
powerio-cli      the powerio command and its terminal interface
powerio-py       the extension behind the Python package
powerio-capi     C ABI 7 for C, C++, Julia, and other bindings
```

`powerio-tx` and `powerio-dist` share `powerio-core` and nothing else, so a
distribution consumer pulls no transmission code. `powerio-prob` is matrix
free. The `powerio` facade re-exports the component crates, and
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
