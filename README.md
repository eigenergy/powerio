# PowerIO


<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/main/docs/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO parses power system case files into a typed `Network`, converts between
formats, and builds sparse matrices and graph representations for solver and analysis
code. Same format writes return retained source text when the reader supports
it; cross format writes report fields the target cannot carry in warnings.

The core is implemented in [Rust](https://rust-lang.org). The
[C ABI](https://github.com/eigenergy/powerio/tree/main/powerio-capi) exposes
the same parser and converter to C, C++, Julia, and other foreign function
interfaces. The Python package and command line interface sit on top of the
same Rust code.

## Overview

When writing back to the source format, PowerIO **returns the original file exactly** when the parser retained it. Cross format conversion obeys **sane defaults** and explicitly emits `Conversion::warnings` for fields the target format cannot represent.

### Formats

The following formats are currently supported:
- [MATPOWER](https://matpower.org/) `.m`
- [PSS/E](https://www.siemens.com/global/en/products/energy/grid-software/planning/pss-software/pss-e.html) `.raw` revisions 33, 34, and 35
- [PowerWorld](https://www.powerworld.com/WebHelp/Content/MainDocumentation_HTML/Case_Formats.htm) `.aux`, plus read only `.pwb` binary cases; `.pwd` display files parse through the separate display API. Vintage coverage and decode evidence live in [docs/powerworld.md](docs/powerworld.md).
- GE PSLF `.epc` power flow cases
- [PowerModels.jl](https://github.com/lanl-ansi/PowerModels.jl) network data JSON
- [egret](https://pypi.org/project/gridx-egret/) `ModelData` JSON
- [pandapower](https://www.pandapower.org/) `pandapowerNet` JSON
- [PyPSA](https://pypsa.org/) static CSV folders
- [GridFM](https://github.com/gridfm) `.parquet`

Distribution networks are supported in wire coordinates via [`powerio-dist`](powerio-dist/):
- [OpenDSS](https://www.epri.com/pages/sa/opendss) `.dss`
- [PowerModelsDistribution.jl](https://github.com/lanl-ansi/PowerModelsDistribution.jl) ENGINEERING data JSON
- The (draft) BMOPF JSON spec and schema of the [IEEE BMOPF task force](https://github.com/frederikgeth/bmopf-report) `.json`

Support for the following formats is under development (see the open pull requests):
- [surge](https://github.com/amptimal/surge) `.surge.json`

Other formats are planned; see the GitHub issues. If a format you need is missing, open an issue or a pull request. All are welcome to contribute to this community project.

### Packages

This repository contains multiple packages. 

```
powerio          # parser, Network model, source retaining writers, converters
powerio-matrix   # sparse matrices, DC sensitivity factors, graph representations
powerio-dist     # multiconductor distribution model, dss/PMD/BMOPF converters
powerio-pkg      # .pio.json compiler package envelope
powerio-cli      # the `powerio` command and ratatui TUI
powerio-py       # PyO3 extension for the Python `powerio` package
powerio-capi     # C ABI for C, C++, Julia, and other foreign function interfaces
PowerIO.jl       # Julia bindings over the C ABI
```

The core [powerio Rust crate](https://crates.io/crates/powerio) keeps parsing
and conversion separate from matrix, TUI, and data frame dependencies. The
[Python package](https://pypi.org/project/powerio/) imports with no required
third party packages; matrix and graph helpers live behind extras.

API docs: <https://eigenergy.github.io/powerio/>.
Language API map: [languages guide](https://eigenergy.github.io/powerio/guides/languages.html).

## Install

```
cargo add powerio
cargo add powerio-matrix
cargo install powerio-cli

pip install powerio
pip install 'powerio[all]'     # scipy, numpy, networkx, polars extras
pip install 'powerio[gridfm]'  # polars for Parquet inspection
pip install 'powerio[pandas]'  # pandas, pyarrow compatibility reads (Python 3.10+)

julia -e 'using Pkg; Pkg.add(url="https://github.com/eigenergy/PowerIO.jl")'
```

## Use

### Rust
```rust
use powerio::{TargetFormat, parse_file};

let parsed = parse_file("case14.m", None)?;
let net = parsed.network;
let conv = net.to_format(TargetFormat::PowerModelsJson)?;

for warning in &conv.warnings {
    eprintln!("conversion warning: {warning}");
}

std::fs::write("case14.json", conv.text)?;
```

### Python
```python
import powerio as pio

case = pio.parse_file("case9.m")
bprime = case.bprime()            # scipy.sparse, needs powerio[matrix]
display = pio.parse_display_file("case.pwd")
raw, warnings = pio.convert_file("case9.m", "psse")
```

### Julia
```julia
using PowerIO

case = parse_file("case9.m")
text = to_matpower(case)
json, warnings = to_format(case, "powermodels-json")
```

### Command line interface (CLI)
```
powerio convert tests/data/case14.m --to psse35 -o case14.raw
powerio convert tests/data/case14.m --to pandapower-json -o case14.pp.json
powerio convert tests/data/case14.m --to pypsa-csv -o pypsa_case
powerio convert pypsa_case --from pypsa-csv --to matpower -o case14.m
powerio convert case.epc --from pslf --to matpower -o case.m
powerio package tests/data/case14.m -o case14.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio gridfm tests/data/case14.m -o out
powerio
```

## Features

### Current Format Fidelity

| reader / writer | MATPOWER | PowerModels JSON | PSS/E | PowerWorld | egret JSON | pandapower JSON |
| --- | --- | --- | --- | --- | --- | --- |
| MATPOWER | original text | full | partial | partial | partial | partial |
| PowerModels JSON | partial | original text | partial | partial | partial | partial |
| PSS/E | full | full | original text | partial | partial | partial |
| PowerWorld | full | full | partial | original text | partial | partial |
| PowerWorld `.pwb` | full | full | partial | partial | partial | partial |
| PSLF `.epc` | partial | partial | partial | partial | partial | partial |
| egret JSON | partial | full | partial | partial | original text | partial |
| pandapower JSON | partial | partial | partial | partial | partial | original text |

`partial` means the target lacks fields present in the source. The writer reports
those cases in `Conversion::warnings`. PowerWorld `.pwb` is read only (no
writer, no retained source): the row shows where its decoded power flow core
lands. PSLF `.epc` reads and writes the power flow core; same format writes
echo retained source text, and unsupported EPC sections are reported as read
warnings. PowerWorld `.pwd` is display data, not a network case, so it is
outside this conversion table and uses `parse_display_file` /
`parse_display_bytes`. The decoded vintages and per field evidence live in
[docs/powerworld.md](docs/powerworld.md).

The distribution matrix (dss, PMD JSON, BMOPF JSON, per fixture) is generated into
[powerio-dist/docs/conversion-matrix.md](https://github.com/eigenergy/powerio/blob/main/powerio-dist/docs/conversion-matrix.md).
Vendored test data keeps its own licenses, documented next to the fixtures
(see [tests/data/dist/README.md](tests/data/dist/README.md)).

PyPSA CSV folders and GridFM Parquet are not in this table only because they
are directory datasets, not single text outputs. Both read and write: PyPSA
with regenerable committed fixtures (`tests/data/pypsa/README.md`), GridFM
with a deliberately lossy read that recovers the power flow core. Known
limits for every format are documented in
the [format fidelity guide](https://eigenergy.github.io/powerio/guides/format-fidelity.html).

### Matrices

The `powerio-matrix` Rust crate derives an `IndexedNetwork` with dense bus indices. It enables you to build common power system matrices with minimal dependencies:

- B' and B'' DCPF and FDPF matrices
- Nodal admittance matrix
- LACPF block matrix
- Signed incidence, weighted Laplacian, and flow map matrices
- PTDF and LODF sensitivity matrices
- Adjacency matrix and `petgraph` graph output
- Matrix Market bundles for low-level OPF solvers
- KKT operators for OPF solvers (experimental)

Current conventions for signs, taps, phase shifts, per unit scaling, reference buses, and line parameters are documented in the [matrices guide](https://eigenergy.github.io/powerio/guides/matrices.html).

### Normalized Form

`Network::to_normalized` derives a post processed copy of a case for solvers:

- powers are in per unit,
- voltage phase angles are in radians, 
- inactive elements are removed, 
- `tap == 0` replaced with `1`,
- surviving buses keep their source bus ids, and
- bus types are made consistent with generator placement and reference buses. 

The normalized copy carries no retained source text, so writing it emits the derived model rather than the original file.

Python exposes the normalized form as `case.to_normalized()`, the C ABI as `pio_normalize`,
and Julia as `to_normalized(case)`.


### C ABI

`powerio-capi` exposes parse, query, conversion, JSON transport, normalization,
and numeric table extraction through `pio_*` functions. The public header is
[powerio-capi/include/powerio.h](https://github.com/eigenergy/powerio/blob/main/powerio-capi/include/powerio.h).
Build with `--features arrow` to enable `pio_to_arrow` over the
[Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html).

### PowerAgent


PowerIO is part of the [PowerAgent](https://github.com/Power-Agent) community. The Python interface for PowerIO currently includes an optional MCP server exposing semantic tools for conversion, saving, summaries, parsing, normalization, matrix outputs, and display data.


```
pip install 'powerio[mcp]'
powerio-mcp
```

The PowerIO MCP server is currently being integrated as the low-level data exchange substrate for the MCP server bundle in [PowerMCP](https://github.com/Power-Agent/PowerMCP). The PowerMCP bundle ships the same
tool surface as PowerIO alongside a wide array of simulator servers, whose bridges ingest the transport directly.

### GridFM (experimental)
PowerIO ships first-class support for the [LF Energy](https://lfenergy.org/projects/gridfm/) open [Grid Foundation Model (GridFM)](https://github.com/gridfm) project. In the command line:

```
powerio gridfm <case> -o <dir>
```

This *writes* the Parquet tables [gridfm-datakit](https://gridfm.github.io/gridfm-datakit/) and
[gridfm-graphkit](https://github.com/gridfm/gridfm-graphkit) consume under `<dir>/<case>/raw/`; several compatible cases
stack by scenario id. 

The `gridfm` feature also supports *reading* a `.parquet` dataset back into a `Network` (`read_gridfm_dataset` in `powerio-matrix`, `pio.read_gridfm` in
Python), so a perturbed training scenario or a GNN predicted state can be extracted and converted back
out in any classical format:

```
powerio convert out/case14/raw --from gridfm --to matpower -o case14.m
```

The `--from gridfm` read functionality is currently lossy. What it recovers, what it drops, and the warnings contract
are in the [format fidelity guide](https://eigenergy.github.io/powerio/guides/format-fidelity.html). Improving `gridfm` read/write functionality is a key priority for the initial development of PowerIO.


## Validation

The Rust test suite covers parsers, writers, format conversion, matrix
builders, and normalization; the C ABI crate carries its own tests, and
`pytest` covers the Python bindings. The benchmark validation suite compares
selected outputs against PowerModels.jl, egret, ExaPowerIO.jl, and pandapower,
and imports PowerIO's PyPSA CSV folders with PyPSA when the optional oracle is
installed.

```
cargo fmt --all --check
cargo test
cargo test -p powerio-capi
cargo clippy --all-targets
pytest python/tests
bash benchmarks/run_validation.sh
```

Benchmark method, environment, and current tables are documented in
[benchmarks/RESULTS.md](https://github.com/eigenergy/powerio/blob/main/benchmarks/RESULTS.md).

## License

PowerIO is distributed under either of:

- [Apache License, Version 2.0](https://github.com/eigenergy/powerio/blob/main/LICENSE-APACHE)
- [MIT license](https://github.com/eigenergy/powerio/blob/main/LICENSE-MIT)


<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/main/docs/assets/powerio-logo.svg"
    alt="PowerIO logo"
    width="120"
  >
</p>
