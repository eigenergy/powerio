# gridforge

`gridforge` is a fast, format-agnostic Rust toolkit that parses power
network data files and emits the **canonical linear-algebra and graph
representations** used in modern power-systems research:

- **Linearized Jacobians**: FDPF B' (shuntless), B'' (with shunts and
  taps), the Talkington LACPF block `[[G, -B], [-B, -G]]`, and (planned)
  LinDist3Flow for unbalanced 3-phase distribution networks.
- **Bus admittance**: full `Y_bus = G + jB` split into real and
  imaginary parts, with optional negation for the positive-Laplacian
  convention expected by sparse Laplacian solvers.
- **Graph view**: a `petgraph::UnGraph<bus_idx, branch_idx>` topology
  for connectivity checks, radial detection, spanning-tree extraction,
  and any of petgraph's algorithms.
- **Output formats**: Matrix Market (`.mtx`, spec-compliant
  lower-triangular for symmetric), NumPy (`.npy`), JSON metadata, and
  (planned) Apache Parquet matching the `gridfm-datakit` schema.

Designed to feed:
- the [Scalable Approximate Cholesky][sac] solver (`.mtx` ingest),
- ML pipelines like [GridFM][gridfm] (`gridfm-datakit` Parquet schema),
- and any sparse-linalg / graph-theory experiment that needs the linear
  core of a power network without paying for a full power-flow solver.

[sac]: https://github.com/UnLochlann/Scalable-Approximate-Cholesky
[gridfm]: https://github.com/orgs/gridfm

## Status

| | status |
| --- | --- |
| MATPOWER `.m` parser (transmission, balanced) | ✓ |
| FDPF B' (XB / BX) | ✓ |
| FDPF B'' (with shunts, taps) | ✓ |
| Y_bus G/B | ✓ |
| LACPF block (Talkington flat-start) | ✓ |
| `petgraph` view + radial / connectivity diagnostics | ✓ |
| Synthetic generator (tree, lattice, pegase-like) | ✓ |
| Interactive `ratatui` TUI | ✓ |
| OpenDSS `.dss` parser | planned |
| LinDist3Flow matrices | planned |
| PSS/E `.raw` parser | planned |
| Parquet output (`gridfm-datakit` compatible) | planned |
| Python bindings via PyO3 | planned |

## Install / build

```bash
cargo build --release
```

## Usage

```bash
gridforge                                       # TUI (default)

gridforge batch -i tests/data -o /tmp/forge \
    --matrices bprime,bdoubleprime,ybus_imag,lacpf \
    --rhs random

gridforge gen --topology lattice --n 1024 -o /tmp/synth

gridforge verify tests/data/case30.m --kind bdoubleprime
```

### TUI

| screen | role | key |
| --- | --- | --- |
| Browse | walk the case directory, multi-select for batch | `Space` |
| Inspect | per-matrix stats, SDDM check, sparsity preview | `Tab` |
| Batch | parallel export with per-case progress bars | `b`,`e` |
| Synth | parametric generator (tree, lattice, pegase-like) | `g`,`e` |

`?` shows the full key reference.

### Output

`gridforge batch` writes spec-compliant Matrix Market files (1-based,
lower-triangular for symmetric) plus a `_meta.json` sidecar capturing
matrix stats, build options, source-file SHA-256, and the gridforge
version that produced the dataset:

```
out/
├── case9_bprime.mtx
├── case9_bdoubleprime.mtx
├── case9_ybus_imag.mtx
├── case9_lacpf.mtx
├── case9_shunt.mtx
├── case9_<kind>_rhs.mtx              (when --rhs random|injection)
└── case9_meta.json
```

## Library API

```rust
use gridforge::{
    parse_matpower_file, build_bprime, build_lacpf, BuildOptions,
    Pipeline, MatrixKind, RhsKind,
};

let mpc = parse_matpower_file("case14.m")?;

// Linear-algebra view
let b_prime = build_bprime(&mpc, &BuildOptions::default())?;

// Graph view (petgraph)
let g = mpc.to_petgraph();
assert!(mpc.connectivity_report().is_single_island());

// Batch export
Pipeline {
    matrices: vec![MatrixKind::BPrime, MatrixKind::Lacpf],
    rhs: RhsKind::Random,
    ..Default::default()
}.run(&mpc, "out/")?;
```

## Conventions

- **Sign convention**: positive-Laplacian (negative off-diagonal,
  positive diagonal, `diag = sum |off-diag|` for B'). Matches the
  Talkington LACPF derivation `B = Aᵀ diag(b) A` and the sparse-solver
  ingest convention.
- **Bus IDs**: MATPOWER 1-based IDs are kept verbatim and mapped to
  dense `[0, n)` indices via `MpcCase::bus_index`. Non-contiguous IDs
  (common in real cases) are handled.
- **Branch conventions**: `tap == 0` is treated as `tap = 1` (MATPOWER
  rule); B' ignores taps and shifts; B'' keeps taps but zeros shifts;
  `Y_bus` keeps both.

## Tests

```
cargo test
```

30 tests cover parser edge cases (multi-line brackets, NaN/Inf, comments,
non-contiguous bus IDs), matrix-builder algebra against a hand-checked
3-bus reference, integration tests against vendored MATPOWER cases
(case9/14/30/57/118), petgraph topology invariants, and TUI snapshot
tests via `ratatui::backend::TestBackend`.

## License

MIT OR Apache-2.0 at your option.

## Acknowledgments

Built around the LACPF (Linear AC Power Flow) derivation by
[Samuel Talkington][talkington] (Georgia Tech ECE), and designed to
complement the [GridFM][gridfm] data + graph kits.

[talkington]: https://github.com/samtalki
