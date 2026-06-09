# PowerIO

Lossless IO and format conversion for power system case files. Reads MATPOWER
`.m`, PSS/E `.raw`, PowerWorld `.aux`, PowerModels JSON, and EGRET JSON into a
format neutral `Network` and writes any of them back. Same format round trips are
byte for byte. Cross format conversion reports what the target drops. Emits the
sparse matrices and graph views a solver needs. Callable from Rust, Python, C,
C++, and Julia. The `powerio` crate has six dependencies and no matrix or solver
stack.

## Workspace

```
powerio          parser, typed Network hub, lossless writer, format converters.
powerio-matrix   sparse matrices + graph views on top of powerio (re-exports it).
powerio-cli      the `powerio` binary: CLI + TUI.
powerio-py       PyO3 extension behind the one `powerio` Python wheel.
powerio-capi     C ABI (`pio_*`), the substrate for C, C++, and Julia.
```

API docs: <https://eigenergy.github.io/powerio/> (moves to docs.rs on a crates.io release).

## Install

```
cargo add powerio            # the parser + converters
cargo install powerio-cli    # the `powerio` command + TUI
pip install powerio          # parse + convert, no extra deps, Python 3.9+
pip install 'powerio[all]'   # + scipy/numpy/networkx for the matrices and graph view
```

## Formats

Every reader produces a `Network` and every writer consumes one; a new format is
one module at the hub, not a converter for each pair.

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
carries survives. **🟨 partial**: the target can't represent some fields (PSS/E and
PowerWorld have no cost curves; EGRET has no HVDC or storage); each dropped field is
reported in `Conversion::warnings`, not dropped silently. Canonical MATPOWER output
omits dcline and storage; the PowerModels writer maps both.

Validated against PowerModels.jl, the EGRET package, ExaPowerIO.jl, and pandapower,
over the full conversion matrix. See [benchmarks/RESULTS.md](benchmarks/RESULTS.md)
and [docs/format-fidelity.md](docs/format-fidelity.md).

## Matrices

From the dense-indexed `IndexedNetwork` view, `powerio-matrix` builds signed
incidence `A`, the weighted Laplacian `L = A diag(b) Aᵀ` and the same matrix
grounded at the slack, B'/B''/`Re(Y_bus)`/`-Im(Y_bus)`, the LACPF block, PTDF/LODF,
the DC-OPF instance bundle, adjacency, and a petgraph view, as Matrix Market or in
memory. The sign, tap, per unit, and DC conventions are in
[docs/matrices.md](docs/matrices.md).

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
# Errors: a bad file is PowerIOParseError, a valid case an operation can't run on
# is PowerIODataError (both subclass PowerIOError); a missing path is OSError.
```

```
powerio convert case14.m --to psse -o case14.raw   # convert
powerio dcopf case30.m -o out                       # DC-OPF instance bundle
powerio sensitivities case30.m -o out               # PTDF + LODF
powerio gridfm case14.m -o out                      # gridfm-datakit Parquet dataset
powerio                                              # TUI
```

## Normalized view

The parsed `Network` is raw and lossless — MATPOWER units (MW/MVAr, degrees),
1-based ids, out-of-service elements kept — so a same-format write echoes the
source byte for byte. `Network::to_normalized` derives the form a solver or ML
pipeline consumes: powers per unit (÷`baseMVA`), angles in radians, `tap 0 → 1`,
out-of-service elements and isolated buses dropped, the survivors reindexed to a
dense 1-based id space, bus types canonicalized (one reference bus; generator
buses PV, the rest PQ). It carries no retained source, so writing it serializes
the per-unit model rather than echoing — a derived product, not a source for
write-back. Python: `case.to_normalized()`. C ABI: `pio_to_normalized`. Parse a
case from memory with no temp file via `parse_str(text, format)` /
`pio_parse_str`.

## GridFM

`powerio gridfm <case> -o <dir>` (the `gridfm` cargo feature) writes the
gridfm-datakit Parquet schema — `bus_data`, `gen_data`, `branch_data`,
`y_bus_data` under `<dir>/<case>/raw/`, which [gridfm-graphkit](https://github.com/gridfm)'s
`HeteroGridDatasetDisk` loads directly. powerio has no solver, so a case is one
snapshot (`scenario 0`): voltages and dispatch are the stored values, branch flows
computed from them.

Pass several inputs — `powerio gridfm <case-0> <case-1> … -o <dir>` — to
row-stack a **scenario batch** into one dataset, keyed by the `scenario` column
(the k-th input is stamped `--scenario` + k). The inputs share a base element set
— the same bus, branch, and generator counts in the same bus order — so the dense
bus index lines up across scenarios; within that, load, dispatch, voltages, branch
status, bus type, and costs may vary, the way datakit's topology variants toggle
line status on a fixed element set. powerio stacks the snapshots, it doesn't
generate them. From Python (the `gridfm` extra): `case.write_gridfm(dir)` and
`powerio.write_gridfm_batch([case0, case1], dir)`.

## C ABI

`powerio-capi` is the C ABI (`pio_*`, header `powerio-capi/include/powerio.h`) for
C, C++, and Julia: parse (from a path, or from memory with `pio_parse_str`), query,
convert, the byte-exact echo, the JSON transport (`pio_to_json`/`pio_from_json`),
the normalized view (`pio_to_normalized`), and the numeric table extractors. `pio_abi_version`
(and the `PIO_ABI_VERSION` header macro) lets a consumer reject a stale or
mismatched library at load; `pio_version` reports the crate version. `--features
arrow` adds `pio_export_arrow`, which exports the raw network tables (bus, branch,
gen, load, shunt) over the [Arrow C Data Interface](https://arrow.apache.org/docs/format/CDataInterface.html).
pyarrow, Arrow.jl, Arrow C++, polars, and DuckDB read a table in process without a
copy or a temp file. It is the in memory form of `pio_to_json`.

## MCP server

powerio ships an optional [MCP](https://modelcontextprotocol.io) server so
LLM agent tooling gets a lossless converter and a case summary over the case
file family. It exposes two tools over stdio — `convert_case` (to a target
format, with fidelity warnings) and `case_summary` (counts, base MVA, source
format, connectivity) — each accepting either a file `path` or inline `content`.

```
pip install 'powerio[mcp]'   # the MCP extra (needs Python 3.10+; the core is 3.9+)
powerio-mcp                  # serve the two tools over stdio
```

## Benchmark

Median parse time, one Apple M-series laptop, release build. Every parser runs in
one process under the same harness, powerio through its C ABI, so all read from
disk alike. Method and full table: [benchmarks/RESULTS.md](benchmarks/RESULTS.md).

<!-- BENCH:speed-main START -->
| case | buses / branches | powerio | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | 1.73 ms | 2.86 ms | 122.2 ms |
| case_ACTIVSg2000 | 2000 / 3206 | 2.07 ms | 2.11 ms | 127.8 ms |
| case9241pegase | 9241 / 16049 | 5.81 ms | 9.15 ms | 553.2 ms |
| case13659pegase | 13659 / 20467 | 8.6 ms | 13.76 ms | 822.2 ms |
| case193k | 192768 / 228574 | 161.9 ms | 174.98 ms | n/a |
<!-- BENCH:speed-main END -->

Across these cases powerio parses faster than ExaPowerIO, 62–96× PowerModels'
parser, and ~14× pandapower's `.m` reader. It round trips byte for byte (verified at
54 MB, 192768 buses); the other two don't. Parse, conversions, and Y_bus are checked
value for value against all three (`benchmarks/run_validation.sh`).

## Roadmap

Tracked in the issues, all on the `Network` hub:

- More formats: PSS/E `.rawx` (v35), IIDM `.xiidm`, UCTE `.uct`, GE EPC `.epc`.
- PSS/E fidelity: 3-winding transformers and non-unit `CZ`/`CW` impedance bases.
- A RAVENS-JSON export sink, so powerio feeds MG-RAVENS.
- A registered [PowerIO.jl](https://github.com/eigenergy/PowerIO.jl) over the C ABI,
  with bridges to PowerModels.jl, ExaModelsPower.jl, and PowerDiff.jl.
- LinDist3Flow matrices.

CIM stays out of scope: a heavier problem owned by the CIM hubs (CIMHub,
MG-RAVENS), which powerio can feed.

## Tests

```
cargo test            # powerio + powerio-matrix, including the all-pairs round trip
cargo run --release -p powerio --example timeparse -- tests/data/case2869pegase.m
```

## License

MIT or Apache 2.0.
