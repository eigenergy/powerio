# netmat

Turns power network case files into structured sparse matrices and graph views for any downstream solver. Parse a case, get the matrix you want — incidence, admittance, Laplacian, PTDF/LODF, FDPF, DC-OPF data — as Matrix Market or NumPy, or in memory. No runtime, no ecosystem to buy into, single binary. The numerical analyst's "give me the matrix, now" tool.

## Inputs

- MATPOWER `.m` (transmission). Done — **lossless**: `parse → write → parse` reproduces the file byte-for-byte, preserving every `mpc.*` field (including ones the typed model doesn't interpret), in-matrix column-header comments, and exact numeric tokens like `7e-05`. `write_matpower` replays the source in ~9 µs; the whole round-trip on case2869pegase is ~2.7 ms. See [benchmarks/RESULTS.md](benchmarks/RESULTS.md).
- OpenDSS `.dss`, PSS/E `.raw`, PowerModels JSON. See issues.

### Versus other parsers

ExaPowerIO.jl is a fast Julia MATPOWER reader but write-only-absent; PowerModels.jl is multi-format but drags in the JuMP/optimization stack and its MATPOWER export is lossy. netmat is the only one that is fast *and* byte-exact round-trip *and* callable from Rust, the CLI, and Python (`pip install netmat`) with no runtime. It captures `bus_name`, HVDC `dcline`, the full generator columns (ramp rates, Pc/Qc, apf), and the reactive-power `gencost` block — fields other lightweight parsers drop.

## Outputs

- Signed incidence `A`, adjacency, weighted Laplacian `L = A diag(b) Aᵀ` (and slack-grounded)
- PTDF and LODF (DC sensitivities)
- B' (FDPF, shuntless) and B'' (FDPF, with shunts and taps)
- `Re(Y_bus)` and `-Im(Y_bus)`
- LACPF block `[[G, -B], [-B, -G]]` (linear AC power flow, flat start)
- DC-OPF instance bundle: incidence `A`, susceptance `b`, the Laplacian `L` and its grounded form, the flow map `B Aᵀ`, generator cost `Q`/`c`, bounds, thermal limits, the generator→bus map `C_g`, and nodal load
- petgraph view + radial / connectivity diagnostics
- Matrix Market (lower triangle, 1 based), NumPy `.npy`, JSON metadata

## Build

```
cargo build --release
```

## Run

```
netmat                                                    # TUI
netmat batch -i tests/data -o out --matrices bprime,bdoubleprime,lacpf --rhs random
netmat gen --topology lattice --n 1024 -o out
netmat verify tests/data/case30.m --kind bdoubleprime
netmat dcopf tests/data/case30.m -o out                  # DC-OPF instance bundle
netmat sensitivities tests/data/case30.m -o out          # PTDF + LODF
```

## TUI keys

| screen   | action                                | key       |
| -------- | ------------------------------------- | --------- |
| Browse   | walk dir, multi select for batch      | `Space`   |
| Inspect  | per matrix stats, sparsity preview    | `Tab`     |
| Batch    | export queue with progress bars       | `b`, `e`  |
| Synth    | tree, lattice, pegase like generator  | `g`, `e`  |

`?` for full key reference.

## Library

```rust
use netmat::{parse_matpower_file, build_bprime, BuildOptions, Pipeline, MatrixKind, RhsKind};

let mpc = parse_matpower_file("case14.m")?;
let b = build_bprime(&mpc, &BuildOptions::default())?;
let g = mpc.to_petgraph();
assert!(mpc.connectivity_report().is_single_island());

// Lossless round-trip: reproduces the source (modulo the trailing newline),
// bus_name and dcline included.
let m = netmat::write_matpower(&mpc);

Pipeline {
    matrices: vec![MatrixKind::BPrime, MatrixKind::Lacpf],
    rhs: RhsKind::Random,
    ..Default::default()
}.run(&mpc, "out/")?;
```

Incidence factorization and DC-OPF instance data:

```rust
use netmat::{build_incidence, build_weighted_laplacian, build_opf_instance,
             DcConvention, Units};

let inc = build_incidence(&mpc, DcConvention::PaperPure)?;   // A, b
let l = build_weighted_laplacian(&inc.a, &inc.b);            // L = A diag(b) Aᵀ
let opf = build_opf_instance(&mpc, &inc, Units::PerUnit)?;   // Q, c, bounds, C_g, p_d
```

## Python

PyO3 bindings expose the parser and every matrix builder as `scipy.sparse` matrices and a networkx graph.

```
pip install netmat            # wheels for Linux / macOS / Windows, Python 3.9+
```

```python
import netmat as nm

case = nm.parse_matpower("tests/data/case9.m")
B = case.bprime()             # scipy.sparse.csr_matrix, the FDPF B'
Y = case.ybus()               # complex csr_matrix, G + jB
A = case.adjacency()
ptdf, lodf = case.ptdf(), case.lodf()
inc = case.incidence()        # inc.A (csr), inc.b, inc.p_shift, inc.branch_of_col
L = case.weighted_laplacian()
g = case.to_networkx()        # needs networkx: pip install 'netmat[networkx]'

case.write_dcopf_bundle("out/")   # the DC-OPF bundle, same as the `dcopf` subcommand
```

Case tables come back as plain dicts, one line from a DataFrame:

```python
import pandas as pd
buses = pd.DataFrame(case.buses)
```

### Benchmark

netmat parses *and* builds matrices; `matpowercaseframes` only parses into DataFrames. On case2869pegase (2869 buses, 4582 branches), release build, Apple M-series, best of 25 runs:

| task                          | time   |
| ----------------------------- | ------ |
| netmat: parse                 | ~2 ms  |
| netmat: parse + Y_bus + B'    | ~5 ms  |
| matpowercaseframes: parse     | ~25 ms |

The full parse + matrix path stays well under the 100 ms target. Reproduce with `python benchmarks/bench_parse.py` after `pip install 'netmat[bench]'`.

Build from source with [maturin](https://www.maturin.rs):

```
maturin develop --release     # into the active venv
pytest python/tests
```

## Conventions

- Positive Laplacian sign convention: negative off diagonal, positive diagonal, `diag = sum |off-diag|` for B'.
- MATPOWER 1 based bus IDs preserved; `MpcCase::bus_index(id)` maps to dense `[0, n)`.
- `tap == 0` ⇒ `tap = 1`. B' ignores taps and shifts; B'' zeros only shifts.
- `BR_B` is already per unit; never divide by `base_mva` again.
- DC-OPF is bus-indexed (`p_g ∈ ℝⁿ`, the paper's convention); generator-space data and `C_g` ride along. Default `b = 1/x` (paper-pure, taps/shifts ignored); `--convention matpower` uses `1/(x·τ)` plus a phase-shift injection. Per-unit by default (`--units native` for raw MW / native cost).

## Tests

```
cargo test
```

Parser edges, matrix algebra against a hand checked 3 bus reference, integration on case9 / 14 / 30 / 57 / 118, petgraph topology invariants, ratatui snapshot tests, and the DC-OPF builders (incidence structure, `L == B'` in the XB scheme, grounded SPD, PTDF reproduces DC flows, bundle round-trip).

## License

MIT or Apache 2.0.
