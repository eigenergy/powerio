# PowerIO


<p align="center">
  <img
    src="https://raw.githubusercontent.com/eigenergy/powerio/60e0126c/docs/src/assets/powerio-hero.png"
    alt="PowerIO format and matrix flow"
    width="720"
  >
</p>

PowerIO 0.10 is the public beta of the 1.0 API. API corrections may land before 1.0.0 as downstream integrations exercise the new design.

PowerIO parses power system data into typed values, converts between supported
formats, and builds sparse matrices and graph data for solvers and analysis
code. One parse reads any supported source and returns a module: the typed
value (a network, a time series, a scenario set, a calculation instance, or a
solution, depending on what the source declares) beside the retained source,
the reader's findings, and the operations that produced it.

```rust,ignore
let module = powerio::parse(Source::open("case9.m")?)?;   // PioModule<PioValue>
let case: PioModule<BalancedNetwork> = powerio::try_into_typed(module)?;
```

Writing an unchanged module back to the format it was read from returns the
original file bytes. Converting to a different format writes what the target
format can hold and reports every dropped field as a coded finding.

`.pio.json` stores one typed value together with the record of how it was
produced: [the version 1 module document](https://powerio.dev/guide/pio-json-schema.html)
carries the source each element came from, durable diagnostics, descriptive
history, and typed time series and scenario values, and every released 0.9
package upgrades one way on read. Files for other tools stay in the format
the tool reads: MATPOWER, PSS/E, OpenDSS, or any other supported format.

The Rust workspace also builds the command line interface, the Python package,
and the [C ABI](https://github.com/eigenergy/powerio/tree/main/powerio-capi).
[PowerIO.jl](https://github.com/eigenergy/PowerIO.jl) uses the C ABI.

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
- [DeepMind OPFData](https://arxiv.org/abs/2406.07234) dataset JSON from
  the published FullTop and N-1 datasets (all grid sizes use the same reader)
- [GridFM](https://github.com/gridfm) `.parquet`

Distribution networks are supported in wire coordinates via [`powerio-dist`](powerio-dist/):
- [OpenDSS](https://www.epri.com/pages/sa/opendss) `.dss`
- [PowerModelsDistribution.jl](https://github.com/lanl-ansi/PowerModelsDistribution.jl) ENGINEERING data JSON
- The (draft) BMOPF JSON spec and schema of the [IEEE BMOPF task force](https://github.com/frederikgeth/bmopf-report) `.json`

If a required format is missing, open an issue with a sample file and its
specification or submit a reader or writer.

### Packages

Each workspace crate owns one layer:

```
powerio-core     # Source, PioModule, diagnostics, errors, time series, destinations
powerio          # entry facade: dynamic value, universal parse, .pio.json documents
powerio-tx       # balanced transmission model, format parsers and writers
powerio-matrix   # generic sparse matrices, sensitivity factors, graph projections
powerio-prob     # complete problem instances; optional matrix projections
powerio-dist     # multiconductor distribution model, dss/PMD/BMOPF converters
powerio-cli      # the `powerio` command and ratatui TUI
powerio-py       # PyO3 extension for the Python `powerio` package
powerio-capi     # C ABI for C, C++, Julia, and other foreign function interfaces
PowerIO.jl       # Julia bindings over the C ABI
```

The [powerio Rust crate](https://crates.io/crates/powerio) keeps parsing and
conversion separate from matrix, TUI, and data frame dependencies. The
[Python package](https://pypi.org/project/powerio/) has no required third party
packages; matrix and graph helpers use optional extras.

Docs site: <https://powerio.dev>.
Language API map: [languages guide](https://eigenergy.github.io/powerio/guide/languages.html).

## Install

```
cargo add powerio
cargo add powerio-matrix
cargo add powerio-prob
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
use powerio::{BalancedNetwork, TargetFormat, write_as};

let module = powerio::parse(powerio_core::Source::open("case14.m")?)?;
let module: powerio_core::PioModule<BalancedNetwork> = powerio::try_into_typed(module)?;
let conv = write_as(&module, TargetFormat::PowerModelsJson)?;

for warning in conv.rendered_diagnostics() {
    eprintln!("conversion warning: {warning}");
}

std::fs::write("case14.json", conv.text)?;
```

### Python
```python
import powerio as pio

case = pio.parse("case9.m", value_type=pio.BalancedNetwork)
bprime = case.value.bprime()      # MATPOWER Bp, scipy.sparse, needs powerio[matrix]
display = pio.parse_display_file("case.pwd")
raw, warnings = pio.convert_file("case9.m", "psse")
```

### Julia
```julia
using PowerIO

case = PowerIO.parse("case9.m")
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
powerio convert example_0.json --from opfdata-json --to matpower -o solved_case.m
powerio module tests/data/case14.m -o case14.pio.json
powerio module goc3_case.json --from goc3-json -o goc3_case.pio.json
powerio verify tests/data/case30.m --kind bdoubleprime
powerio dcopf tests/data/case30.m -o out
powerio sensitivities tests/data/case30.m -o out --solver auto --drop-tolerance 1e-10
powerio gridfm tests/data/case14.m -o out
powerio
```

## Features

### Current Format Fidelity

Every network reader lowers to `BalancedNetwork`. The table separates writing back to
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
| PyPSA CSV folder | yes | yes | directory output without text echo | PyPSA import validator checks the exported static components |
| GO Challenge 3 JSON | yes | source echo only | byte exact retained source | first interval maps to the static power flow core; `.pio.json` documents retain time series as operating points |
| Surge JSON | yes | yes | byte exact retained source | versioned JSON network body; unsupported source sections stay in retained source or warnings |
| DeepMind OPFData JSON | yes | source echo only | byte exact retained source | each extracted FullTop or N-1 document maps to a solved snapshot with element counts read from that document; source-only initial values and omitted identifiers/defaults are warned |
| GridFM Parquet | yes | yes | directory output, lossy read | recovers the power flow core for conversion back to classical formats |

PowerWorld `.pwd` carries display data rather than a network case, so it is
outside this conversion table and uses `parse_display_file` /
`parse_display_bytes`. The
decoded vintages and per field evidence are maintainer notes at
[`powerio-tx/src/format/powerworld/FORMAT.md`](powerio-tx/src/format/powerworld/FORMAT.md).

The distribution matrix (dss, PMD JSON, BMOPF JSON, per fixture) is generated
under `powerio-dist/docs/`. Vendored test data keeps its own licenses next to
the fixtures under `tests/data/dist/`.

Known limits for every format are documented in the
[format fidelity guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html).

### Matrices

`powerio-matrix` derives an `IndexedNetwork` with dense bus indices and builds:

- MATPOWER Bp/Bpp FDPF matrices
- DC bus susceptance matrix `L = A diag(b) A^T` and flow map matrices
- Nodal admittance matrix
- LACPF block matrix
- Signed incidence matrix
- PTDF and LODF sensitivity matrices, with dense and iterative solver paths
- Streamed CLI PTDF/LODF writes for iterative sensitivity exports
- Adjacency matrix and `petgraph` graph output

`powerio-prob` builds matrix free problem instances: DC OPF and AC OPF input
data plus the GO Challenge 3 AC SCUC instance, and never reaches `powerio-matrix`
(a `cargo tree` gate enforces it). `powerio-matrix` depends on `powerio-prob`
and adds the sparse projections and DC OPF Matrix Market bundles over those
instances.

Current conventions for signs, taps, phase shifts, per unit scaling, reference buses, and line parameters are documented in the [matrices guide](https://eigenergy.github.io/powerio/guide/matrices.html).

### Normalized Form

`BalancedNetwork::to_normalized` returns a derived solver view:

- powers use per unit;
- voltage phase angles use radians;
- inactive elements are removed;
- `tap == 0` becomes `1`;
- surviving buses keep their source bus IDs;
- bus types reflect generator placement and reference buses.

The normalized copy carries no retained source text, so writing it emits the derived model rather than the original file.

Python exposes the normalized form as `case.to_normalized()`, the C ABI as `pio_balanced_network_normalize`,
and Julia as `to_normalized(case)`.


### C ABI

`powerio-capi` exposes parse, query, conversion, JSON transport, normalization,
`.pio.json` document handles, and numeric table extraction through `pio_*`
functions. The public header is
[powerio-capi/include/powerio.h](https://github.com/eigenergy/powerio/blob/main/powerio-capi/include/powerio.h).
Build with `--features arrow` to enable `pio_balanced_network_to_arrow` over the
[Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html),
and add `--features matrix` for sparse matrix COO tables. Matrix Arrow ABI v1
is COO plus explicit `matrix_bus` and `matrix_branch` axis map tables; language
bindings assemble native sparse matrix types on their side. The optional
`prob` feature exposes the matrix free DC branch data spans.

### PowerAgent


The optional MCP server exposes conversion, saving, summaries, parsing,
normalization, matrix outputs, and display data.


```
pip install 'powerio[mcp]'
powerio-mcp
```

`python -m powerio.mcp` and the `powerio-mcp` console script are consumer entry points and do not move without a version bump.

MCP clients can keep a case in `.pio.json` document JSON through the `module`
transport:

```python
parsed = parse(path="case9.m", transport="module")
stored = parsed["module_json"]
summary(module_json=stored)
matrix("bprime", module_json=stored)
save(out_path="case9.raw", to_format="psse", module_json=stored)
diagnostics(stored)
```

[PowerMCP](https://github.com/Power-Agent/PowerMCP) bundles these tools with
simulator servers and bridge tools.

### `.pio.json` documents

`.pio.json` documents store one typed value — a network, a time or scenario
collection, a problem instance, or a solution — beside the module's common
records: source descriptors, source maps, diagnostics, history, and namespaced
extensions. Released 0.9 packages upgrade one way on read; a 0.9 GO Challenge
3 package with `operating_points` upgrades to a balanced operating point time
series, and `export_state` materializes one static item from it.

Rust reads and writes the document with `powerio::stored::read_module` and
`write_module`, Python with `powerio.PioModule` (and `powerio.parse`,
which loads either stored generation), the C ABI with `pio_module_*`, and
the CLI writes documents with `powerio module`.

### GridFM

The `gridfm` command writes Parquet datasets used by the
[LF Energy GridFM project](https://github.com/gridfm):

```
powerio gridfm <case> -o <dir>
```

The command writes the tables consumed by
[gridfm-datakit](https://gridfm.github.io/gridfm-datakit/) and
[gridfm-graphkit](https://github.com/gridfm/gridfm-graphkit) under
`<dir>/<case>/raw/`. Compatible cases can be stacked by scenario ID.

`read_gridfm_dataset` in `powerio` and `pio.read_gridfm` in Python
reconstruct a `BalancedNetwork` from a dataset. The reconstructed network can be
written to any supported balanced case format:

```
powerio convert out/case14/raw --from gridfm --to matpower -o case14.m
```

The `--from gridfm` path is lossy. The
[format fidelity guide](https://eigenergy.github.io/powerio/guide/format-fidelity.html)
lists the recovered fields and warnings.


## Validation

The Rust test suite covers parsers, writers, format conversion, matrix
builders, and normalization; the C ABI crate carries its own tests, and
`pytest` covers the Python bindings. The benchmark validation suite compares
selected outputs against PowerModels.jl, egret, ExaPowerIO.jl, and pandapower,
and imports PowerIO's PyPSA CSV folders with PyPSA. Install the oracle stack
from `evals/validation/requirements.txt` into the same Python 3.11+ venv that holds
the local `powerio` wheel.

```
cargo fmt --all --check
cargo test
cargo test -p powerio-capi
bash scripts/ci-clippy.sh
pytest python/tests
bash evals/validation/run_validation.sh
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
