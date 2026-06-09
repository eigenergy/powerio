# PowerIO

<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/main/docs/assets/powerio-logo.png"
    alt="PowerIO logo"
    width="120"
  >
</p>

PowerIO reads power system case files into a typed `Network`, writes them back,
converts between common formats, and builds the sparse matrices and graph views
used by analysis and solver code.

Supported formats:

- [MATPOWER](https://matpower.org/) `.m`
- [PSS/E](https://www.siemens.com/global/en/products/energy/grid-software/planning/pss-software/pss-e.html) `.raw` revision 33
- [PowerWorld](https://www.powerworld.com/WebHelp/Content/MainDocumentation_HTML/Case_Formats.htm) `.aux`
- [PowerModels.jl](https://github.com/lanl-ansi/PowerModels.jl) network data JSON
- [egret](https://pypi.org/project/gridx-egret/) `ModelData` JSON

Writing back to the source format returns the original text when the parser
retained it. Cross format conversion emits `Conversion::warnings` for fields the
target cannot represent.

<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/main/docs/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

## Packages

```
powerio          parser, Network model, source retaining writers, converters
powerio-matrix   sparse matrices, DC sensitivity factors, graph views
powerio-cli      the `powerio` command and ratatui TUI
powerio-py       PyO3 extension for the Python `powerio` package
powerio-capi     C ABI for C, C++, Julia, and other foreign function interfaces
PowerIO.jl       Julia bindings over the C ABI
```

API docs: <https://eigenergy.github.io/powerio/>.
Language API map: [docs/languages.md](https://github.com/eigenergy/powerio/blob/main/docs/languages.md).

## Install

```
cargo add --git https://github.com/eigenergy/powerio powerio
cargo add --git https://github.com/eigenergy/powerio powerio-matrix
cargo install --git https://github.com/eigenergy/powerio powerio-cli

pip install powerio
pip install 'powerio[all]'   # scipy, numpy, networkx, polars extras
pip install 'powerio[gridfm]'  # polars for Parquet inspection
pip install 'powerio[pandas]'  # pandas, pyarrow compatibility reads (Python 3.10+)

julia -e 'using Pkg; Pkg.add(url="https://github.com/eigenergy/PowerIO.jl")'
```

## Use

```rust
use powerio::{TargetFormat, parse_file};

let net = parse_file("case14.m")?;
let conv = net.to_format(TargetFormat::PowerModelsJson);

for warning in &conv.warnings {
    eprintln!("conversion warning: {warning}");
}

std::fs::write("case14.json", conv.text)?;
```

```python
import powerio as pio

case = pio.parse_file("case9.m")
bprime = case.bprime()            # scipy.sparse, needs powerio[matrix]
raw, warnings = pio.convert_file("case9.m", "psse")
```

```julia
using PowerIO

case = parse_file("case9.m")
text = to_matpower(case)
json, warnings = to_format(case, "powermodels-json")
```

```
powerio convert tests/data/case14.m --to psse -o case14.raw
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio gridfm tests/data/case14.m -o out
powerio
```

## Format Fidelity

| reader / writer | MATPOWER | PowerModels JSON | PSS/E | PowerWorld | egret JSON |
| --- | --- | --- | --- | --- | --- |
| MATPOWER | original text | full | partial | partial | partial |
| PowerModels JSON | full | original text | partial | partial | partial |
| PSS/E | full | full | original text | partial | partial |
| PowerWorld | full | full | partial | original text | partial |
| egret JSON | full | full | partial | partial | original text |

`partial` means the target lacks fields present in the source. The writer reports
those cases in `Conversion::warnings`. Known limits are documented in
[docs/format-fidelity.md](https://github.com/eigenergy/powerio/blob/main/docs/format-fidelity.md).

## Matrices

`powerio-matrix` derives an `IndexedNetwork` with dense bus indices and builds:

- B' and B'' FDPF matrices
- `Re(Y_bus)` and `-Im(Y_bus)`
- LACPF block matrix
- signed incidence, weighted Laplacian, and flow map
- PTDF and LODF dense sensitivity matrices
- DC OPF Matrix Market bundle
- adjacency matrix and `petgraph` view

Conventions for signs, taps, phase shifts, per unit scaling, reference buses, and
DC susceptance are in
[docs/matrices.md](https://github.com/eigenergy/powerio/blob/main/docs/matrices.md).

## Normalized View

`Network::to_normalized` derives a solver oriented copy of a case: powers in per
unit, angles in radians, inactive elements removed, `tap == 0` replaced with `1`,
surviving buses reindexed to a dense 1-based id space, and bus types made
consistent with generator placement and reference buses. It carries no retained
source text, so writing it emits the derived model rather than the original file.

Python exposes it as `case.to_normalized()`, the C ABI as `pio_to_normalized`,
and Julia as `to_normalized(case)`.

## GridFM

`powerio gridfm <case> -o <dir>` writes the Parquet tables consumed by
[gridfm-datakit](https://gridfm.github.io/gridfm-datakit/) and
`gridfm-graphkit`: `bus_data`, `gen_data`, `branch_data`, and `y_bus_data` under
`<dir>/<case>/raw/`. A case file is one scenario. Passing several compatible
cases stacks them by scenario id.

## C ABI

`powerio-capi` exposes parse, query, conversion, JSON transport, normalization,
and numeric table extraction through `pio_*` functions. The public header is
[powerio-capi/include/powerio.h](https://github.com/eigenergy/powerio/blob/main/powerio-capi/include/powerio.h).
Build with `--features arrow` to enable `pio_export_arrow` over the
[Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html).

## Optional MCP Server

The Python package includes an optional MCP server with `convert_case` and
`case_summary` tools.

```
pip install 'powerio[mcp]'
powerio-mcp
```

## Validation

The Rust test suite covers parsers, writers, format conversion, matrix builders,
and normalization; the C ABI crate carries its own tests (it is outside the
default members, so it needs an explicit `-p powerio-capi`), and `pytest` covers
the Python bindings. The benchmark validation suite compares selected outputs
against PowerModels.jl, egret, ExaPowerIO.jl, and pandapower.

```
cargo fmt --all --check
cargo test
cargo test -p powerio-capi
cargo clippy --all-targets
pytest python/tests
bash benchmarks/run_validation.sh
```

Benchmark method, environment, and current tables are in
[benchmarks/RESULTS.md](https://github.com/eigenergy/powerio/blob/main/benchmarks/RESULTS.md).

## License

PowerIO is distributed under either of:

- [Apache License, Version 2.0](https://github.com/eigenergy/powerio/blob/main/LICENSE-APACHE)
- [MIT license](https://github.com/eigenergy/powerio/blob/main/LICENSE-MIT)
