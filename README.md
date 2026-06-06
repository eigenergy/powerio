# caseio

The fast, lossless parser and data layer for power-system case files. Parse a MATPOWER `.m` case, work with a typed model, and write it back out **byte-for-byte**. Dependency-light on purpose, so other tools can take it as a parser without dragging in a matrix or solver stack.

Two crates in this workspace:

- **`caseio`** — the parser, the typed `MpcCase` (buses, branches, generators, storage, HVDC), the retained source text, and the lossless writer. Five dependencies, no sparse-matrix or TUI baggage.
- **`casemat`** — sparse matrices and graph views built on caseio: B'/B''/Y_bus, PTDF/LODF, incidence, weighted Laplacian, the LACPF block, adjacency, and the DC-OPF instance bundle, plus a CLI/TUI. Also the `casemat` Python package.

## Lossless round-trip

`parse → write → parse` reproduces the source file byte-for-byte — every `mpc.*` field (including ones the typed model doesn't interpret), in-matrix column-header comments, and exact numeric tokens like `7e-05` that an `f64`-based writer would mangle. The parse retains the original source text and the writer echoes it, so round-trip costs no extra parse pass. This is the property other lightweight parsers lack: ExaPowerIO has no writer, and PowerModels' MATPOWER export is lossy.

## Benchmark

Median parse time (`parse_matpower`), same machine (Apple M-series, release build); all three return identical bus/branch counts. Full table and method in [benchmarks/RESULTS.md](benchmarks/RESULTS.md).

| case | buses / branches | **caseio** | ExaPowerIO.jl | PowerModels.jl |
| --- | --- | --- | --- | --- |
| case2869pegase | 2869 / 4582 | **1.90 ms** | 3.86 ms | 133 ms |
| case_ACTIVSg2000 | 2000 / 3206 | **2.08 ms** | 3.06 ms | 148 ms |
| case9241pegase | 9241 / 16049 | **5.62 ms** | 9.85 ms | 620 ms |
| case13659pegase | 13659 / 20467 | **8.34 ms** | 15.1 ms | 893 ms |
| case193k | 192768 / 228574 | **169 ms** | 194 ms | — |

caseio is 25–70× faster than PowerModels' parser and faster than ExaPowerIO (the focused Julia reader) on every case — ~1.5–2× on the pegase cases, 7–15% on the synthetic US cases — scaling linearly to a 193k-bus / 54 MB file, and it does this on the same single parse path that gives a byte-exact round-trip. caseio is the only one of the three that is lossless, round-trips byte-for-byte (verified at 193k buses), and is callable from Rust, the CLI, and Python with no runtime. Full table: [benchmarks/RESULTS.md](benchmarks/RESULTS.md).

## caseio: parse and write

```rust
use caseio::{parse_matpower_file, write_matpower};

let case = parse_matpower_file("case14.m")?;        // typed MpcCase
assert!(case.connectivity_report().is_single_island());
let bus0 = case.buses[0].name.as_deref();           // bus_name, dclines, ...

let m = write_matpower(&case);                       // reproduces the source
```

`caseio` depends only on `thiserror`, `num-complex`, `petgraph`, `serde`, and `fast-float` — light enough to vendor as a parser.

## casemat: matrices on top

```rust
use casemat::{parse_matpower_file, build_bprime, build_incidence, build_weighted_laplacian,
              BuildOptions, DcConvention};

let mpc = parse_matpower_file("case14.m")?;          // caseio, re-exported
let b = build_bprime(&mpc, &BuildOptions::default())?;
let inc = build_incidence(&mpc, DcConvention::PaperPure)?;   // A, b
let l = build_weighted_laplacian(&inc.a, &inc.b);            // L = A diag(b) Aᵀ
```

Outputs: signed incidence `A`, adjacency, weighted Laplacian and its slack-grounded form, B'/B'', `Re(Y_bus)`/`-Im(Y_bus)`, PTDF/LODF, the LACPF block, the DC-OPF instance bundle, and a petgraph view — as Matrix Market, NumPy `.npy`, or in memory.

### CLI

```
casemat                                                  # TUI
casemat batch -i tests/data -o out --matrices bprime,bdoubleprime
casemat dcopf tests/data/case30.m -o out                 # DC-OPF instance bundle
casemat sensitivities tests/data/case30.m -o out         # PTDF + LODF
```

### Python

```
pip install casemat            # wheels for Linux / macOS / Windows, Python 3.9+
```

```python
import casemat as cm
case = cm.parse_matpower("case9.m")
B = case.bprime()             # scipy.sparse.csr_matrix
Y = case.ybus()               # complex csr_matrix, G + jB
g = case.to_networkx()
```

## Conventions

- Positive Laplacian: negative off-diagonal, positive diagonal, `diag = sum |off-diag|` for B'.
- MATPOWER 1-based bus IDs preserved; `MpcCase::bus_index(id)` maps to dense `[0, n)`.
- `tap == 0` ⇒ `tap = 1`. B' ignores taps and shifts; B'' zeros only shifts.
- `BR_B` is already per unit; never divide by `base_mva` again.
- DC-OPF is bus-indexed (`p_g ∈ ℝⁿ`); default `b = 1/x` (paper-pure), `--convention matpower` uses `1/(x·τ)` plus a phase-shift injection.

## Roadmap

PSS/E `.raw` (v33–v35), OpenDSS, and PowerModels-JSON parsers; pandapower / PyPSA adapters; and a C ABI so Julia/C++ can consume `caseio` directly. See the issues.

## Tests

```
cargo test            # caseio + casemat
cargo run --release -p caseio --example timeparse -- tests/data/case2869pegase.m
```

## License

MIT or Apache 2.0.
