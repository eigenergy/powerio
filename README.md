# gridforge

Parses power network case files and emits sparse matrices and graph views for solver and ML pipelines.

## Inputs

- MATPOWER `.m` (transmission). Done.
- OpenDSS `.dss`, PSS/E `.raw`, PowerModels JSON. See issues.

## Outputs

- B' (FDPF, shuntless)
- B'' (FDPF, with shunts and taps)
- `Re(Y_bus)` and `-Im(Y_bus)`
- LACPF block `[[G, -B], [-B, -G]]` (Talkington flat start linearization)
- petgraph view + radial / connectivity diagnostics
- Matrix Market (lower triangle, 1 based), NumPy `.npy`, JSON metadata

## Build

```
cargo build --release
```

## Run

```
gridforge                                                    # TUI
gridforge batch -i tests/data -o out --matrices bprime,bdoubleprime,lacpf --rhs random
gridforge gen --topology lattice --n 1024 -o out
gridforge verify tests/data/case30.m --kind bdoubleprime
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
use gridforge::{parse_matpower_file, build_bprime, BuildOptions, Pipeline, MatrixKind, RhsKind};

let mpc = parse_matpower_file("case14.m")?;
let b = build_bprime(&mpc, &BuildOptions::default())?;
let g = mpc.to_petgraph();
assert!(mpc.connectivity_report().is_single_island());

Pipeline {
    matrices: vec![MatrixKind::BPrime, MatrixKind::Lacpf],
    rhs: RhsKind::Random,
    ..Default::default()
}.run(&mpc, "out/")?;
```

## Conventions

- Positive Laplacian sign convention: negative off diagonal, positive diagonal, `diag = sum |off-diag|` for B'.
- MATPOWER 1 based bus IDs preserved; `MpcCase::bus_index(id)` maps to dense `[0, n)`.
- `tap == 0` ⇒ `tap = 1`. B' ignores taps and shifts; B'' zeros only shifts.
- `BR_B` is already per unit; never divide by `base_mva` again.

## Tests

```
cargo test
```

30 tests: parser edges, matrix algebra against a hand checked 3 bus reference, integration on case9 / 14 / 30 / 57 / 118, petgraph topology invariants, ratatui snapshot tests.

## License

MIT or Apache 2.0.
