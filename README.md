# PowerIO

Lossless IO and format conversion for power system case files. Parse MATPOWER
`.m`, PSS/E `.raw`, PowerWorld `.aux`, PowerModels JSON, and EGRET JSON into one
format neutral `Network`; write any of them back (same-format round trips are byte
for byte); convert between them with explicit fidelity reporting; and emit the
sparse matrices and graph views a solver needs. The same Rust core is callable
from Rust, Python, C/C++, and Julia. The core crate has six dependencies and no
matrix or solver stack.

## Workspace

```
powerio          parser, typed Network hub, lossless writer, format converters.
powerio-matrix   sparse matrices + graph views on top of powerio (re-exports it).
powerio-cli      the `powerio` binary: CLI + TUI.
powerio-py       PyO3 extension behind the one `powerio` Python wheel.
powerio-capi     C ABI (`pio_*`), the substrate for C, C++, and Julia.
```

Full API docs are on [docs.rs/powerio](https://docs.rs/powerio) and
[docs.rs/powerio-matrix](https://docs.rs/powerio-matrix).

## Install

```
cargo add powerio            # the parser + converters
cargo install powerio-cli    # the `powerio` command + TUI
pip install powerio          # zero-dependency parse + convert, Python 3.9+
pip install 'powerio[all]'   # + scipy/numpy/networkx for the matrices and graph view
```

## Formats

Every reader produces a `Network` and every writer consumes one, so a new format
is one module at the hub, not an N×M matrix of pairwise converters.

**Readers and writers**: MATPOWER `.m`, PowerModels JSON, PSS/E `.raw` (v33),
PowerWorld `.aux`, and EGRET JSON.

Legend: 🟩 byte-exact · 🟦 full · 🟨 partial (drops are logged in `Conversion::warnings`)

| reader ↓ \ writer → | MATPOWER | PowerModels JSON | PSS/E | PowerWorld | EGRET JSON |
| --- | --- | --- | --- | --- | --- |
| **MATPOWER** | 🟩 | 🟦 | 🟨 | 🟨 | 🟨 |
| **PowerModels JSON** | 🟦 | 🟩 | 🟨 | 🟨 | 🟨 |
| **PSS/E** | 🟦 | 🟦 | 🟩 | 🟨 | 🟨 |
| **PowerWorld** | 🟦 | 🟦 | 🟨 | 🟩 | 🟨 |
| **EGRET JSON** | 🟦 | 🟦 | 🟨 | 🟨 | 🟩 |

**🟩 byte-exact**: writing back to the source format reproduces the file verbatim,
comments and exact tokens like `7e-05` included. **🟦 full**: every field the source
carries survives. **🟨 partial**: the target cannot represent some fields (PSS/E and
PowerWorld have no cost curves; EGRET has no HVDC or storage), and each dropped
field is reported in `Conversion::warnings`, not dropped silently. Two target
caveats fold into this: canonical MATPOWER output omits dcline and storage, and the
PowerModels writer maps them best-effort.

Every reader and writer is validated against an independent tool, PowerModels.jl,
the EGRET package, ExaPowerIO.jl, and pandapower, over the full conversion matrix.
See [benchmarks/RESULTS.md](benchmarks/RESULTS.md) and
[docs/format-fidelity.md](docs/format-fidelity.md).

## Matrices

`powerio-matrix` builds, from the dense-indexed `IndexedNetwork` view: signed
incidence `A`, the weighted Laplacian `L = A diag(b) Aᵀ` and its slack-grounded
form, B'/B''/`Re(Y_bus)`/`-Im(Y_bus)`, the LACPF block, PTDF/LODF, the DC-OPF
instance bundle, adjacency, and a petgraph view, as Matrix Market or in memory.
The sign, tap, per unit, and DC-OPF conventions are documented in the
[crate docs](https://docs.rs/powerio-matrix).

## Use

```rust
use powerio::{parse_matpower_file, write_as, TargetFormat};

let net = parse_matpower_file("case14.m")?;                // MATPOWER → neutral hub
let conv = write_as(&net, TargetFormat::PowerModelsJson);  // → PowerModels JSON
for w in &conv.warnings { eprintln!("fidelity: {w}"); }    // what couldn't be represented
std::fs::write("case14.json", conv.text)?;
```

```python
import powerio
case = powerio.parse("case9.m")        # format inferred from the extension
B = case.bprime()                      # scipy.sparse FDPF B'  (needs powerio[matrix])
raw, warnings = powerio.convert("case9.m", "psse")
```

```
powerio convert case14.m --to psse -o case14.raw   # convert
powerio dcopf case30.m -o out                       # DC-OPF instance bundle
powerio sensitivities case30.m -o out               # PTDF + LODF
powerio gridfm case14.m -o out                      # gridfm-datakit Parquet dataset
powerio                                              # TUI
```

## GridFM

`powerio gridfm <case> -o <dir>` (the `gridfm` cargo feature) writes the
gridfm-datakit Parquet schema — `bus_data`, `gen_data`, `branch_data`,
`y_bus_data` under `<dir>/<case>/raw/` — so [gridfm-graphkit](https://github.com/gridfm)'s
`HeteroGridDatasetDisk` trains on powerio output directly. A parsed case is one
snapshot (`scenario 0`): voltages and dispatch are the case's stored values, and
branch flows are computed from them. Per-scenario expansion is future work
(issue #14).

## Benchmark

Median parse time, one Apple M-series laptop, release build, all timed in one
process under the same harness, powerio through its C ABI, so every parser reads
from disk alike. Full table and method in
[benchmarks/RESULTS.md](benchmarks/RESULTS.md).

| case | buses / branches | powerio | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | 1.73 ms | 2.86 ms | 122.2 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.07 ms | 2.11 ms | 127.8 ms |
| case9241pegase | 9241 / 16049 | 5.81 ms | 9.15 ms | 553.2 ms |
| case13659pegase | 13659 / 20467 | 8.6 ms | 13.76 ms | 822.2 ms |
| case193k | 192768 / 228574 | 161.9 ms | 174.98 ms | n/a |

powerio is faster than ExaPowerIO on every case measured, 62–96× PowerModels'
parser, and ~14× pandapower's `.m` reader. It is also the only one of the three
that round trips byte for byte (verified at 54 MB / 192768 buses) and is callable
from Rust, the CLI, Python, and C/Julia with no runtime. Parse, conversions, and
Y_bus are validated value for value against all three (`benchmarks/run_validation.sh`).

## Roadmap

Tracked in the issues, all over the `Network` hub:

- Broader format coverage: PSS/E `.rawx` (v35), IIDM `.xiidm`, UCTE `.uct`, GE EPC `.epc`.
- PSS/E fidelity: 3-winding transformers and non-unit `CZ`/`CW` impedance bases.
- A RAVENS-JSON export sink (positioning PowerIO as an ingest backend for MG-RAVENS).
- A registered [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl) over the C ABI, with
  native bridges to PowerModels.jl, ExaModelsPower.jl, and PowerDiff.jl (scaffolded there now).
- LinDist3Flow matrices, and the scenario-batch path that stacks many perturbed
  scenarios into the gridfm Parquet tables (issue #14).

CIM stays out of scope; it's a heavier problem owned by the CIM hubs (CIMHub,
MG-RAVENS), which PowerIO can feed as an ingest layer.

## Tests

```
cargo test            # powerio + powerio-matrix, including the all-pairs round trip
cargo run --release -p powerio --example timeparse -- tests/data/case2869pegase.m
```

## License

MIT or Apache 2.0.
