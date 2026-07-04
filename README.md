# PowerIO


<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/60e0126c/docs/src/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO is compiler infrastructure for power systems. Case files from a dozen
transmission and distribution formats parse into typed intermediate
representations (IR). Once parsed, you can perform explicit, recorded operations, like normalization, validation, and lowering.
You can compile/write the case back into any supported target format, sparse matrix families, and ML model formats. 

The `.pio.json` package serves as a unified network payload under declared [schema versions](https://powerio.dev/guide/pio-json-schema.html),
which records where the data came from, and how it maps back to the original source file. 
Furthermore, the package contains structured diagnostics, validation, and replayable operating points, enabling many downstream tasks.

Data fidelity and interoperability is the primary goal of the PowerIO project. Writing a parsed file back to its own format returns
the original text when the reader kept it. Converting to another format writes 
the modeled electrical data and reports every field the target cannot carry in
`Conversion::warnings`.

The core of PowerIO is written in [Rust](https://rust-lang.org). The Rust version is used to create the Python package and the command line interface, both of which sit in this repo. The Rust implementation also enables the creation of a [C ABI](https://github.com/eigenergy/powerio/tree/main/powerio-capi), which exposes PowerIO capabilities to C, C++, [Julia](https://github.com/eigenergy/PowerIO.jl), and other foreign function interfaces (FFIs). 

## Overview

PowerIO is a community infrastructure project intended to serve all developers working on electric power systems. Everyone is welcome to use, build upon, and contribute to the PowerIO infrastructure project. 

### Formats

Supported formats:
- [MATPOWER](https://matpower.org/) `.m`
- [PSS/E](https://www.siemens.com/global/en/products/energy/grid-software/planning/pss-software/pss-e.html) `.raw` revisions 33, 34, and 35
- [PowerWorld](https://www.powerworld.com/WebHelp/Content/MainDocumentation_HTML/Case_Formats.htm) `.aux`, plus read only `.pwb` binary cases; `.pwd` display files parse through the separate display API. Behavior and limits are in the [format fidelity guide](https://powerio.dev/guide/format-fidelity.html).
- GE PSLF `.epc` power flow cases
- [PowerModels.jl](https://github.com/lanl-ansi/PowerModels.jl) network data JSON
- [egret](https://pypi.org/project/gridx-egret/) `ModelData` JSON
- [pandapower](https://www.pandapower.org/) `pandapowerNet` JSON
- [PyPSA](https://pypsa.org/) static CSV folders
- [ARPA-E GO Competition Challenge 3](https://gocompetition.energy.gov/) JSON input data
- [surge](https://github.com/amptimal/surge) `.surge.json`
- [GridFM](https://github.com/gridfm) `.parquet`
- PowerIO JSON snapshots (`powerio-json`) and `.pio.json` compiler packages

Distribution networks are supported in wire coordinates via [`powerio-dist`](powerio-dist/):
- [OpenDSS](https://www.epri.com/pages/sa/opendss) `.dss`
- [PowerModelsDistribution.jl](https://github.com/lanl-ansi/PowerModelsDistribution.jl) ENGINEERING data JSON
- The (draft) BMOPF JSON spec and schema of the [IEEE BMOPF task force](https://github.com/frederikgeth/bmopf-report) `.json`

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

Docs site: <https://powerio.dev>.
Language API map: [languages guide](https://eigenergy.github.io/powerio/guide/languages.html).

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
powerio convert case.surge.json --from surge-json --to matpower -o case.m
powerio convert goc3_case.json --from goc3-json --to matpower -o case.m
powerio package tests/data/case14.m -o case14.pio.json
powerio package goc3_case.json --from goc3-json -o goc3_case.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out
powerio gridfm tests/data/case14.m -o out
powerio
```

## Features

### Current Format Fidelity

Every network reader lowers to `Network`. The table separates writing back to
the original file type from converting to a different file type.

| file type | read | write | writing back to the original file type | converting to another file type |
| --- | --- | --- | --- | --- |
| MATPOWER `.m` | yes | yes | byte exact retained source | canonical MATPOWER blocks; warnings for fields MATPOWER cannot carry |
| PowerModels JSON | yes | yes | byte exact retained source | per unit structured data checked against PowerModels.jl |
| PSS/E `.raw` | yes | yes | byte exact only when writing the source revision | power flow core; revision downgrade and unsupported records are warned |
| PowerWorld `.aux` | yes | yes | byte exact retained source | power flow core; PowerWorld only fields are projected or warned |
| PowerWorld `.pwb` | yes | no | n/a | read only binary case; decoded core converts through every text writer |
| PSLF `.epc` | yes | yes | byte exact retained source | power flow core; unsupported EPC sections are read warnings |
| egret JSON | yes | yes | byte exact retained source | ModelData shape checked against egret and PowerModels.jl |
| pandapower JSON | yes | yes | byte exact retained source | pandapower import validator checks counts and Y_bus |
| PyPSA CSV folder | yes | yes | directory output, not text echo | PyPSA import validator checks the exported static components |
| GO Challenge 3 JSON | yes | source echo only | byte exact retained source | first interval maps to the static power flow core; `.pio.json` packages retain time series as operating points |
| Surge JSON | yes | yes | byte exact retained source | versioned JSON network body; unsupported source sections stay in retained source or warnings |
| GridFM Parquet | yes | yes | directory output, deliberately lossy read | recovers the power flow core for conversion back to classical formats |
| PowerIO JSON | yes | yes | structured model snapshot, not byte exact source echo | lossless for `Network` fields except retained source text |

PowerWorld `.pwd` is display data, not a network case, so it is outside this
conversion table and uses `parse_display_file` / `parse_display_bytes`. The
decoded vintages and per field evidence are maintainer notes at
[`powerio/src/format/powerworld/FORMAT.md`](powerio/src/format/powerworld/FORMAT.md).

The distribution matrix (dss, PMD JSON, BMOPF JSON, per fixture) is generated
under `powerio-dist/docs/`. Vendored test data keeps its own licenses next to
the fixtures under `tests/data/dist/`.

Known limits for every format are documented in the
[format fidelity guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html).

### Matrices

The `powerio-matrix` Rust crate derives an `IndexedNetwork` with dense bus indices. It enables you to build common power system matrices with minimal dependencies:

- B' and B'' DCPF and FDPF matrices
- Nodal admittance matrix
- LACPF block matrix
- Signed incidence, weighted Laplacian, and flow map matrices
- PTDF and LODF sensitivity matrices
- Adjacency matrix and `petgraph` graph output
- Matrix Market bundles for OPF solvers
- KKT operators for OPF solvers (experimental)

Current conventions for signs, taps, phase shifts, per unit scaling, reference buses, and line parameters are documented in the [matrices guide](https://eigenergy.github.io/powerio/guide/matrices.html).

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
`.pio.json` package handles, and numeric table extraction through `pio_*`
functions. The public header is
[powerio-capi/include/powerio.h](https://github.com/eigenergy/powerio/blob/main/powerio-capi/include/powerio.h).
Build with `--features arrow` to enable `pio_to_arrow` over the
[Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html),
and add `--features matrix` for sparse matrix COO tables.

### PowerAgent


PowerIO is part of the [PowerAgent](https://github.com/Power-Agent) community. The Python package includes an optional MCP server with tools for conversion, saving, summaries, parsing, normalization, matrix outputs, and display data.


```
pip install 'powerio[mcp]'
powerio-mcp
```

MCP clients can keep a case in the `.pio.json` package transport:

```python
parsed = parse(path="case9.m", transport="package")
pkg = parsed["package_json"]
summary(package_json=pkg)
matrix("bprime", package_json=pkg)
save(out_path="case9.raw", to_format="psse", package_json=pkg)
diagnostics(pkg)
```

The PowerMCP bundle in [PowerMCP](https://github.com/Power-Agent/PowerMCP) uses the same PowerIO tool surface alongside simulator servers and bridge tools.

### Compiler Packages

`.pio.json` packages wrap one balanced or multiconductor payload with provenance,
source maps, diagnostics, validation, summaries, lowering history, optional
derived metadata, and optional `operating_points`. A GO Challenge 3 package
stores the static first interval in `model` and the full replayable time series
in `operating_points`; materializing one point returns a static package with the
updates applied and the series cleared.

Rust uses `powerio_pkg::NetworkPackage`, Python uses the `powerio.Package`
class, the C ABI uses `pio_package_*`, and the CLI writes packages with
`powerio package`.

### GridFM (experimental)
PowerIO writes datasets for the [LF Energy](https://lfenergy.org/projects/gridfm/) open [Grid Foundation Model (GridFM)](https://github.com/gridfm) project. In the command line:

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

The `--from gridfm` read path is lossy. What it recovers, what it drops, and its warning behavior
are in the [format fidelity guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html).


## Validation

The Rust test suite covers parsers, writers, format conversion, matrix
builders, and normalization; the C ABI crate carries its own tests, and
`pytest` covers the Python bindings. The benchmark validation suite compares
selected outputs against PowerModels.jl, egret, ExaPowerIO.jl, and pandapower,
and imports PowerIO's PyPSA CSV folders with PyPSA. Install the oracle stack
from `benchmarks/requirements.txt` into the same Python 3.11+ venv that holds
the local `powerio` wheel.

```
cargo fmt --all --check
cargo test
cargo test -p powerio-capi
cargo clippy --all-targets
pytest python/tests
bash benchmarks/run_validation.sh
```

Benchmark method, environment, and current tables are documented in the
[performance guide](https://eigenergy.github.io/powerio/guide/performance.html).

## License

PowerIO is distributed under either of:

- [Apache License, Version 2.0](https://github.com/eigenergy/powerio/blob/main/LICENSE-APACHE)
- [MIT license](https://github.com/eigenergy/powerio/blob/main/LICENSE-MIT)


<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/main/docs/src/assets/powerio-logo.svg"
    alt="PowerIO logo"
    width="120"
  >
</p>
