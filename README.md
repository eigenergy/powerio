# PowerIO

Fast, lossless parser and format converter for power system case files. Parse a MATPOWER `.m` case into a typed, format-neutral model, write it back byte-for-byte, convert between formats through a neutral hub, and emit the sparse matrices and graph views a solver needs. Light on dependencies, so other tools and languages can embed it without a matrix or solver stack.

PowerIO is the data layer: one fast, lossless reader/writer every tool and language can share. It parses MATPOWER, PSS/E, PowerWorld, and PowerModels JSON into a single format-neutral `Network`, converts between them with explicit fidelity reporting (never a silent drop), and emits the matrices and graph views solvers consume. The same Rust core is callable from Rust, Python, C/C++, and Julia, so a case reads identically everywhere. PowerIO does the IO and the linear algebra views, then hands clean data to the solvers and frameworks you already use rather than competing with them.

What's distinct:

- **Byte-exact same-format round-trip.** `parse → write → parse` reproduces the source, comments and exact numeric tokens included. PowerModels and pandapower are lossy on write; ExaPowerIO has no writer.
- **Hub topology.** N readers and M writers meet at `Network`, so a new format is one module, not N×M converters.
- **Dependency-light core**, no solver or matrix stack required, with a polyglot surface (Rust crate, one Python wheel, C ABI, Julia package) sharing a single parser.
- **Fastest of the set measured** — faster than ExaPowerIO on every case, ~50–70× PowerModels, ~10× pandapower — with validation harnesses against all three.

## Workspace

```
powerio          parser, typed Network hub, lossless writer, format converters. Six light deps.
powerio-matrix   sparse matrices + graph views on top of powerio (re-exports it).
powerio-cli      the `powerio` binary + TUI.
powerio-py       PyO3 extension → the one `powerio` Python wheel.
powerio-capi     C ABI → the polyglot substrate (C, C++, Julia).
```

`cargo add powerio` gives the light parser; `cargo install powerio-cli` gives the `powerio` command.

## Lossless round-trip

`parse → write → parse` reproduces the source byte-for-byte — every `mpc.*` field (including ones the typed model doesn't interpret), in-matrix column-header comments, and exact tokens like `7e-05` that an `f64`-based writer would mangle. The parser keeps the original text and the writer echoes it, so the round-trip costs no extra parse pass. ExaPowerIO has no writer; PowerModels' MATPOWER export is lossy.

## Convert

Every reader produces a format-neutral `Network` (first-class buses, loads, shunts, branches, generators, storage, HVDC) and every writer consumes it — N readers × M writers, not pairwise. The fidelity contract is two-tier:

- **Same-format round-trip is byte-exact.** Each reader keeps its source text; writing back to that format echoes it.
- **Cross-format keeps maximal fidelity with itemized loss.** Anything the target can't represent is reported in `Conversion::warnings`, never dropped silently.

PowerModels JSON and PSS/E are validated against `PowerModels.jl` (which reads both): the writers value-for-value on the vendored cases, and the PSS/E reader against real PTI `.raw` files (`benchmarks/validate_powermodels.jl`, `benchmarks/validate_psse.jl`, both need Julia). The all-pairs harness (`powerio/tests/roundtrip_formats.rs`) pins core preservation, reader∘writer idempotence, and the byte-exact echo in plain `cargo test`.

PowerIO covers the transmission planning / OPF interchange formats — the bus/branch/gen/load/shunt family — not CIM or distribution feeders, which the CIM hubs (CIMHub, MG-RAVENS) own. It fits as the ingest/conversion layer feeding such a hub or a solver.

**Readers**: MATPOWER `.m`, PowerModels JSON, PSS/E `.raw` (v33), PowerWorld `.aux`. **Writers**: those plus EGRET JSON. Every format reads to and writes from the same `Network`, so a new format is one reader/writer at the hub.

| reader ↓ \ writer → | MATPOWER | PowerModels JSON | EGRET JSON | PSS/E | PowerWorld |
| --- | --- | --- | --- | --- | --- |
| **MATPOWER** | byte-exact | validated vs PowerModels.jl | schema-faithful | core + warnings | core + warnings |
| **PowerModels JSON** | core | byte-exact | schema-faithful | core + warnings | core + warnings |
| **PSS/E** | core | core | schema-faithful | byte-exact | core + warnings |
| **PowerWorld** | core | core | schema-faithful | core + warnings | byte-exact |

*byte-exact* = same-format source echo; *core* = bus/branch/gen/load/shunt preserved, with anything the target can't hold reported in `warnings`.

```rust
use powerio::{parse_matpower_file, write_as, TargetFormat};

let net = parse_matpower_file("case14.m")?;                // MATPOWER → neutral hub
let conv = write_as(&net, TargetFormat::PowerModelsJson);  // → PowerModels JSON
for w in &conv.warnings { eprintln!("fidelity: {w}"); }    // what couldn't be represented
std::fs::write("case14.json", conv.text)?;
```

From the CLI and Python (input format inferred from the extension):

```
powerio convert case14.m --to psse -o case14.raw       # → PSS/E .raw
powerio convert case14.raw --to powermodels-json       # PSS/E → PowerModels JSON, to stdout
```

```python
import powerio
r = powerio.convert("case14.m", "egret-json")
print(r.warnings)            # fields EGRET couldn't represent
open("case14.json", "w").write(r.text)
```

## Benchmark

Median parse time, same machine (Apple M-series, release build), all three timed in one process under the same harness — PowerIO through its C ABI, so every parser reads the file from disk alike. Full table and method in [benchmarks/RESULTS.md](benchmarks/RESULTS.md).

| case | buses / branches | **powerio** | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | **1.78 ms** | 2.72 ms | 121 ms |
| case_ACTIVSg2000 | 2000 / 3206 | **2.07 ms** | 2.07 ms | 122 ms |
| case9241pegase | 9241 / 16049 | **5.67 ms** | 8.94 ms | 558 ms |
| case13659pegase | 13659 / 20467 | **8.57 ms** | 13.1 ms | 781 ms |
| case193k | 192768 / 228574 | **158 ms** | 169 ms | — |

PowerIO is 50–70× faster than PowerModels' parser and faster than (or tied with) ExaPowerIO, the focused Julia reader, on every case here — ~35% on the pegase cases and ~2–12% on the ACTIVSg / SyntheticUSA / US cases, where it parses and keeps the `gentype` / `genfuel` / `bus_name` cell arrays ExaPowerIO drops. It's also ~10× faster than pandapower's `.m` reader. And it's the only one of the three that is lossless, round-trips byte-for-byte (verified at 193k buses), and is callable from Rust, the CLI, Python, and C/Julia with no runtime; its parse, conversions, and Y_bus are validated value for value against all three (`benchmarks/run_validation.sh`).

## powerio: parse and write

```rust
use powerio::{parse_matpower_file, write_matpower, IndexedNetwork};

let net = parse_matpower_file("case14.m")?;          // typed Network
assert!(IndexedNetwork::new(&net).connectivity_report().is_single_island());
let bus0 = net.buses[0].name.as_deref();             // bus_name, dclines, ...

let m = write_matpower(&net);                        // reproduces the source
```

`powerio` depends only on `thiserror`, `num-complex`, `petgraph`, `serde`, `serde_json`, and `fast-float`.

## powerio-matrix: matrices on top

```rust
use powerio_matrix::{parse_matpower_file, IndexedNetwork, build_bprime, build_incidence,
                     build_weighted_laplacian, BuildOptions, DcConvention};

let net = parse_matpower_file("case14.m")?;          // powerio, re-exported
let g = IndexedNetwork::new(&net);                   // dense-indexed analysis view
let b = build_bprime(&g, &BuildOptions::default())?;
let inc = build_incidence(&g, DcConvention::PaperPure)?;     // A, b
let l = build_weighted_laplacian(&inc.a, &inc.b);           // L = A diag(b) Aᵀ
```

Outputs: signed incidence `A`, adjacency, weighted Laplacian and its slack-grounded form, B'/B'', `Re(Y_bus)`/`-Im(Y_bus)`, PTDF/LODF, the LACPF block, the DC-OPF instance bundle, and a petgraph view — as Matrix Market or in memory.

### CLI

```
powerio                                                  # TUI
powerio batch -i tests/data -o out --matrices bprime,bdoubleprime
powerio dcopf tests/data/case30.m -o out                 # DC-OPF instance bundle
powerio sensitivities tests/data/case30.m -o out         # PTDF + LODF
```

### Python

```
pip install powerio                # zero-dep parse + convert; wheels for Linux/macOS/Windows, 3.9+
pip install 'powerio[all]'         # + scipy/numpy/networkx for matrices and the graph view
```

```python
import powerio
case = powerio.parse_matpower("case9.m")
B = case.bprime()             # scipy.sparse.csr_matrix   (needs powerio[matrix])
Y = case.ybus()               # complex csr_matrix, G + jB
g = case.to_networkx()        # needs powerio[graph]
```

`import powerio` and parse/write/convert pull in nothing but the interpreter; scipy/numpy/networkx are optional extras (`powerio[matrix]`, `[graph]`, `[all]`), and a missing one raises a clear ImportError.

## Conventions

- Positive Laplacian: negative off-diagonal, positive diagonal, `diag = sum |off-diag|` for B'.
- MATPOWER 1-based bus IDs preserved; `IndexedNetwork::bus_index(id)` maps to dense `[0, n)`.
- `tap == 0` ⇒ `tap = 1`. B' ignores taps and shifts; B'' zeros only shifts.
- `BR_B` is already per unit; never divide by `base_mva` again.
- DC-OPF is bus-indexed (`p_g ∈ ℝⁿ`); default `b = 1/x` (paper-pure), `--convention matpower` uses `1/(x·τ)` plus a phase-shift injection.

## Roadmap

More over the `Network` hub, tracked in the issues:

- An EGRET-JSON **reader** (the writer is done) to make EGRET two-way.
- Broader format coverage: PSS/E `.rawx` (JSON v35), IIDM `.xiidm`, UCTE `.uct`, GE EPC `.epc`.
- PSS/E fidelity: 3-winding transformers, non-unit `CZ`/`CW` impedance bases, switched shunts; and an external validation for PowerWorld `.aux`.
- A RAVENS-JSON export sink (and positioning PowerIO as a fast ingest backend for MG-RAVENS).
- An MCP `convert`/`validate` tool over the Python binding.
- A registered `PowerIO.jl` over the C ABI, with native bridges to PowerModels.jl, ExaModelsPower.jl, and PowerDiff.jl.

The C ABI (`powerio-capi`) for C/C++/Julia is done — it carries a `pio_to_json`/`pio_from_json` transport alongside the dense table extractors, and it's what the benchmark harness times PowerIO through. CIM stays out of scope; it's a heavier problem owned by the CIM hubs.

## Tests

```
cargo test            # powerio + powerio-matrix
cargo run --release -p powerio --example timeparse -- tests/data/case2869pegase.m
```

## License

MIT or Apache 2.0.
