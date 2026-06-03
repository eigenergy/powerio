# netmat

Turns power network case files into structured sparse matrices and graph views for any downstream solver. Parse a case, get the matrix you want — incidence, admittance, Laplacian, PTDF/LODF, FDPF, DC-OPF data — as Matrix Market or NumPy, or in memory. No runtime, no ecosystem to buy into, single binary. The numerical analyst's "give me the matrix, now" tool.

## Inputs

- MATPOWER `.m` (transmission). Done.
- OpenDSS `.dss`, PSS/E `.raw`, PowerModels JSON. See issues.

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
